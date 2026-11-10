import { invoke } from '@tauri-apps/api/core';
import { sameIp, samePort } from './util';

// 在 Tauri 里一律走真接口；只有浏览器直接打开（没有 Tauri 桥）才进演示模式。
export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
export const isDemo = !isTauri;

export interface NetworkCheck {
  ok: boolean;
  checkedAt: string;
  ip?: string;
  error?: string;
}

export interface UsageBucket {
  /** 桶名：5h、7d、7d_fable、7d_opus、7d_sonnet…（令牌查到的是响应头里的名字） */
  key: string;
  /** 0～100 */
  utilization: number;
  resetsAt: string | null;
  status?: 'allowed' | 'rejected' | string;
}

/** expired：普通登录的令牌刚过期，等 Claude Code 自己续期（换号台从不续期） */
export type UsageState = 'ok' | 'limited' | 'invalid' | 'expired' | 'error' | 'neterr';

export interface UsageCheck {
  ok: boolean;
  checkedAt: string;
  state: UsageState;
  error?: string;
  buckets?: UsageBucket[];
  /** 额外用量（超出订阅额度后按量付费）：响应头 overage-status / overage-disabled-reason */
  overage?: { status: string | null; disabledReason: string | null } | null;
  /** claude-code：平时用 Claude Code 时顺带读到；native-probe：手动点查询，用 Claude Code 发了一次请求 */
  source?: 'claude-code' | 'native-probe';
}

// ———— 切换：先预览要退出的窗口，确认后退出 → 换号 → 各自恢复 ————

export interface SwitchWindow {
  windowId: string;
  name: string;
  cwd: string;
  sessionId: string;
  pid: number;
  kind: 'foreground' | 'background';
  permissionMode: string;
  /** 这个窗口现在用的号 */
  accountName: string;
  hasBackgroundTasks: boolean;
  canFocus?: boolean;
  blockers?: string[];
  model?: string | null;
  effort?: string | null;
}

export interface ManagedWindow extends SwitchWindow {
  status: string;
  host: string;
  canFocus: boolean;
}

export interface SwitchPlan {
  planId: string;
  createdAt: string;
  /** false：这台电脑走原来的切换（只影响新开的终端） */
  supported: boolean;
  windows: SwitchWindow[];
  /** 有任何一条就不能切换 */
  blockers: string[];
  globalBlockers?: string[];
}

/** preserve：每个窗口按原来的权限恢复 */
export type RestorePermission = 'preserve' | 'default' | 'bypassPermissions';

export interface SwitchFailure {
  windowId: string;
  cwd: string;
  sessionId: string;
  error: string;
}

export interface SwitchResult {
  operationId: string;
  switched: boolean;
  switchedAt: string | null;
  restored: number;
  failed: SwitchFailure[];
  errors: string[];
}

export interface SwitchProgress {
  stage: string;
  text: string;
  completed: number;
  total: number;
}

/** 套餐由用户手填（令牌没有读账号资料的权限）。 */
export type Plan = 'pro' | 'max5' | 'max20' | '';

export interface Account {
  id: string;
  email?: string;
  name: string;
  plan: Plan;
  tokenTail: string;
  org: string | null;
  proxy: string;
  expectedIp: string;
  note: string;
  addedAt: string;
  current: boolean;
  lastNetworkCheck: NetworkCheck | null;
  lastUsageCheck: UsageCheck | null;
}

export interface NormalLogin {
  email: string | null;
  /** 本机 /login 号的组织编号，用来认出和某个令牌是同一个号 */
  org: string | null;
  current: boolean;
  /** 普通登录读得到套餐；max = 是 Max 但分不出 5×/20× */
  plan: Plan | 'max' | 'ultra';
  /** 订阅状态（active / canceled …），没查过为 null */
  subscriptionStatus: string | null;
  /** 付款方式，如 apple_subscription */
  billingType: string | null;
  /** 固定出口；空 = 还没测过，第一次查额度时记下 */
  expectedIp: string;
  lastUsageCheck: UsageCheck | null;
}

export interface Conflict {
  source: string;
  key: string;
}

/** 换号台登记的普通登录账号：各走一次官方登录，各有自己的固定出口。id 可直接给 previewSwitch / checkUsage。 */
export type OrdinaryAccount = NormalLogin & { id: string; name: string; proxy: string; note?: string; sourceTokenId?: string | null; lastNetworkCheck?: NetworkCheck | null };

export interface AppState {
  version: string;
  dataRoot: string;
  defaultProxy: string;
  accounts: Account[];
  normal: NormalLogin;
  /** 旧后端没有这个字段，按空列表处理 */
  ordinaryAccounts?: OrdinaryAccount[];
  conflicts: Conflict[];
}

/** 添加普通账号：后端跑原生官方登录，给出登录链接（回调到本机）；用户在走这个号出口的浏览器里授权 */
export interface NormalLoginStatus {
  status: 'waiting' | 'complete' | 'failed';
  error?: string;
  /** 原生 CLI 生成的官方登录链接；生成前没有 */
  loginUrl?: string;
}

export interface AccountInput {
  id?: string;
  name: string;
  plan: Plan;
  token?: string;
  proxy: string;
  expectedIp: string;
  note: string;
}

export interface Api {
  getState(): Promise<AppState>;
  listManagedWindows(): Promise<ManagedWindow[]>;
  focusManagedWindow(windowId: string): Promise<void>;
  editCard(input: AccountInput & {id: string; updatePair: boolean}): Promise<void>;
  addAccount(input: AccountInput & { token: string }): Promise<{ account: Account; replaced: boolean }>;
  updateAccount(input: AccountInput & { id: string }): Promise<Account>;
  replaceToken(id: string, token: string): Promise<Account>;
  deleteAccount(id: string): Promise<{ fellBackToNormal: boolean }>;
  switchTo(id: string): Promise<{ switchedAt: string }>;
  useNormal(): Promise<{ switchedAt: string }>;
  /** id 为 null 是切回普通登录 */
  previewSwitch(id: string | null): Promise<SwitchPlan>;
  refreshSwitch(planId: string): Promise<SwitchPlan>;
  executeSwitch(planId: string, permissionMode: RestorePermission): Promise<SwitchResult>;
  getSwitchProgress(planId: string): Promise<SwitchProgress>;
  retryRestore(operationId: string, windowId: string): Promise<SwitchResult>;
  /** 用这个号自己的凭据和代理，经 Claude Code 发一次请求读额度（会用少量额度）；id 也可以是普通账号的 */
  checkUsage(id: string): Promise<UsageCheck>;
  /**
   * 开始登记一个普通账号：先定好它的固定出口；只登记，不切换当前号。
   * sourceTokenId：这个号已有令牌卡（同一个个人账号）时传原卡 id，端口和 IP 必须沿用原卡；
   * 后端登录后核对同组织、个人订阅、同端口同 IP，两张卡都保留。新号不传。
   */
  beginNormalLogin(input: { name: string; proxy: string; expectedIp: string; sourceTokenId?: string; note?: string; plan?: Plan }): Promise<{ loginId: string }>;
  getNormalLogin(loginId: string): Promise<NormalLoginStatus>;
  cancelNormalLogin(loginId: string): Promise<void>;
  /** 用本机 /login 的令牌只读查额度和套餐（不续期） */
  checkNormalUsage(): Promise<UsageCheck>;
  checkNetwork(id: string): Promise<NetworkCheck>;
  /** 不用令牌，测这个代理现在从哪个 IP 出去 */
  detectExit(proxy: string): Promise<{ ip: string }>;
  /** 导出 CPA 认证文件：id 为 null 是普通登录。后端弹保存框并直接写文件，只回路径；取消回 null */
  exportCpa(id: string | null): Promise<{ path: string } | null>;
  openDataFolder(): Promise<void>;
  openHelp(): Promise<void>;
}

const real: Api = {
  getState: () => invoke('get_state'),
  listManagedWindows: () => invoke('list_managed_windows'),
  focusManagedWindow: (windowId) => invoke('focus_managed_window', {windowId}),
  editCard: ({updatePair, ...input}) => invoke('edit_card', {input, updatePair}),
  addAccount: ({ name, plan, token, proxy, expectedIp, note }) => invoke('add_account', { input: { name, plan, token, proxy, expectedIp, note } }),
  updateAccount: ({ id, name, plan, proxy, expectedIp, note }) => invoke('update_account', { input: { id, name, plan, proxy, expectedIp, note } }),
  replaceToken: (id, token) => invoke('replace_token', { id, token }),
  deleteAccount: (id) => invoke('delete_account', { id }),
  switchTo: (id) => invoke('switch_to', { id }),
  useNormal: () => invoke('use_normal'),
  previewSwitch: (id) => invoke('preview_switch', { id }),
  refreshSwitch: (planId) => invoke('refresh_switch', { planId }),
  executeSwitch: (planId, permissionMode) => invoke('execute_switch', { planId, permissionMode }),
  getSwitchProgress: (planId) => invoke('get_switch_progress', { planId }),
  retryRestore: (operationId, windowId) => invoke('retry_restore', { operationId, windowId }),
  checkUsage: (id) => invoke('check_usage', { id }),
  beginNormalLogin: (input) => invoke('begin_normal_login', { ...input }),
  getNormalLogin: (loginId) => invoke('get_normal_login', { loginId }),
  cancelNormalLogin: (loginId) => invoke('cancel_normal_login', { loginId }),
  checkNormalUsage: () => invoke('check_normal_usage'),
  checkNetwork: (id) => invoke('check_network', { id }),
  detectExit: (proxy) => invoke('detect_exit', { proxy }),
  exportCpa: (id) => invoke('export_cpa', { id }),
  openDataFolder: () => invoke('open_data_folder'),
  openHelp: () => invoke('open_help'),
};

/** Tauri 命令失败时 Err 里就是给用户看的中文；其他异常不展示原文。 */
export function errorText(error: unknown): string {
  if (typeof error === 'string' && error.trim()) return error;
  return '操作失败，请重试';
}

// ———— 演示模式：只在内存里，不落盘、不联网、不改环境变量 ————

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const now = () => new Date().toISOString();
const ago = (min: number) => new Date(Date.now() - min * 60_000).toISOString();
const later = (min: number) => new Date(Date.now() + min * 60_000).toISOString();
const hex = (n = 32) => Array.from({ length: n }, () => '0123456789abcdef'[Math.floor(Math.random() * 16)]).join('');
const DEMO_ROOT = 'C:\\Users\\演示\\AppData\\Local\\ClaudeAccountManager';

function usage(minAgo: number, buckets: [string, number, number][]): UsageCheck {
  const limited = buckets.some(([k, u, r]) => (k === '5h' || k === '7d') && u >= 100 && r > 0);
  return {
    ok: true,
    checkedAt: ago(minAgo),
    state: limited ? 'limited' : 'ok',
    source: 'claude-code',
    overage: { status: 'rejected', disabledReason: 'org_level_disabled' },
    buckets: buckets.map(([key, utilization, resetIn]) => ({
      key, utilization, resetsAt: later(resetIn), status: utilization >= 100 ? 'rejected' : 'allowed',
    })),
  };
}

function acct(p: Partial<Account> & Pick<Account, 'name' | 'proxy' | 'expectedIp'>): Account {
  return {
    id: hex(), plan: 'pro', tokenTail: hex(6), org: `${hex(8)}-${hex(4)}`, note: '', addedAt: ago(60 * 24 * 9), current: false,
    lastNetworkCheck: null, lastUsageCheck: null, ...p,
  };
}

// The offline browser preview uses the fictional accounts in the approved design.
function sampleRoute(port: number, last: number) {
  return {proxy:'http://127.0.0.1:' + port, expectedIp:'203.0.113.' + last};
}
function sampleLogin(card: Omit<OrdinaryAccount,'org'|'billingType'|'subscriptionStatus'|'current'>): OrdinaryAccount {
  return {...card,org:'demo-org-'+card.id,billingType:null,subscriptionStatus:'active',current:card.id==='atlas'};
}
let demo_ordinary: OrdinaryAccount[] = [
  sampleLogin({id:'atlas', name:'主力工作', email:'atlas@example.com', note:'日常开发与评审', plan:'max5', ...sampleRoute(31001,11), lastUsageCheck:usage(2,[['5h',86,102],['7d',43,6150]])}),
  sampleLogin({id:'birch', name:'协作账号', email:'birch@example.com', note:'团队协作', plan:'pro', ...sampleRoute(31002,27), lastUsageCheck:usage(2,[['5h',91,-22],['7d',18,4160]])}),
  sampleLogin({id:'lumen', name:'长文研究', email:'lumen@example.com', note:'长文档阅读', plan:'max20', ...sampleRoute(31004,59), lastUsageCheck:usage(2,[['7d',32,1230]])}),
  sampleLogin({id:'maple', name:'个人备用', email:'maple@example.com', note:'个人项目', plan:'max5', ...sampleRoute(31005,75), lastUsageCheck:usage(2,[['5h',26,135],['7d',35,9000]])}),
  sampleLogin({id:'reed', name:'临时协作', email:'reed@example.com', note:'第一次使用后显示额度', plan:'pro', ...sampleRoute(31008,123), lastUsageCheck:null}),
  sampleLogin({id:'coral', name:'备用开发', email:'coral@example.com', note:'原端口异常，备用本机端口 31110', plan:'pro', ...sampleRoute(31010,155), lastUsageCheck:usage(2,[['5h',20,115],['7d',17,3920]]), lastNetworkCheck:{ok:false,checkedAt:ago(2),error:'出口不符'}}),
];
let demo_accounts: Account[] = [
  acct({id:'cedar', name:'实验账号', email:'cedar@example.com', note:'模型对照实验', plan:'pro', ...sampleRoute(31003,43), lastUsageCheck:usage(2,[['5h',78,8],['7d',51,3620]])}),
  acct({id:'elm', name:'文档整理', email:'elm@example.com', note:'整理项目文档', plan:'pro', ...sampleRoute(31006,91), lastUsageCheck:usage(2,[['5h',100,14],['7d',39,4280]])}),
  acct({id:'grove', name:'周末项目', email:'grove@example.com', note:'周末协作', plan:'max20', ...sampleRoute(31007,107), lastUsageCheck:usage(2,[['7d',55,19]])}),
  acct({id:'dune', name:'归档账号', email:'dune@example.com', note:'需要更换令牌', plan:'pro', ...sampleRoute(31009,139), lastUsageCheck:{ok:false,checkedAt:ago(2),state:'invalid',error:'令牌失效'}}),
];
let demo_normal: NormalLogin = {email:null,org:null,current:false,plan:'',subscriptionStatus:null,billingType:null,expectedIp:'',lastUsageCheck:null};

interface DemoLogin { plan?: Plan; note?: string; name: string; proxy: string; expectedIp: string; at: number; cancelled: boolean; done?: boolean; source?: Account }
const demo_logins = new Map<string, DemoLogin>();
/** 演示：登录完成，登记成普通账号（只登记一次）；原卡模式沿用原卡的组织和出口，原卡保留 */
function demoCommit(l: DemoLogin) {
  if (l.done) return;
  l.done = true;
  demo_ordinary = [...demo_ordinary, {
    id: 'ord-' + hex(12), name: l.name, email: `${hex(6)}@example.com`, org: l.source?.org ?? `${hex(8)}-${hex(4)}`, current: false,
    plan: l.plan || l.source?.plan || 'pro', note:l.note || '', sourceTokenId:l.source?.id, subscriptionStatus: 'active', billingType: 'stripe_subscription', expectedIp: l.expectedIp,
    lastUsageCheck: null, proxy: l.proxy,
  }];
}
const publicOrdinary = (o: OrdinaryAccount): OrdinaryAccount => ({ ...o });
function demoTarget(id: string): { name: string; blocked?: string } {
  const t = demo_accounts.find((x) => x.id === id);
  if (t) return { name: t.name };
  const o = demo_ordinary.find((x) => x.id === id);
  if (o) return { name: o.name };
  throw '找不到这个账号';
}
function demoCurrentName() {
  return demo_accounts.find((x) => x.current)?.name ?? demo_ordinary.find((x) => x.current)?.name ?? '普通登录';
}

// 演示：开着的 Claude 窗口
const DEMO_WINDOWS: Omit<SwitchWindow, 'accountName'>[] = [
  {windowId:'w1',name:'换号台界面调整',cwd:'D:\\Projects\\AccountManager',sessionId:'2bc3b669',pid:18244,kind:'foreground',permissionMode:'default',hasBackgroundTasks:false},
  {windowId:'w2',name:'整理项目文档',cwd:'D:\\Projects\\Docs',sessionId:'8e5ab294',pid:20716,kind:'foreground',permissionMode:'default',hasBackgroundTasks:true},
  {windowId:'w3',name:'运行回归测试',cwd:'D:\\Projects\\Tests',sessionId:'971f52ba',pid:9480,kind:'foreground',permissionMode:'bypassPermissions',hasBackgroundTasks:false},
];
const demo_plans = new Map<string, { target: string | null; at: number }>();
const demo_progress = new Map<string, SwitchProgress>();
// 演示：每次切换的累计结果（和真后端一样，重试后回整份结果）
const demo_ops = new Map<string, { result: SwitchResult; attempts: Record<string, number> }>();
const copyResult = (r: SwitchResult): SwitchResult => ({ ...r, failed: r.failed.map((f) => ({ ...f })), errors: [...r.errors] });

function demoFind(id: string) {
  const a = demo_accounts.find((x) => x.id === id);
  if (!a) throw '找不到这个账号';
  return a;
}

function validateProxy(proxy: string) {
  if (!/^http:\/\/(127\.0\.0\.1|localhost|\[::1\]):\d+\/?$/.test(proxy.trim())) throw '只支持本机 HTTP 代理';
}

/** 演示：每个代理端口固定对应一个出口 */
function demoExit(proxy: string) {
  const port = Number(/:(\d+)\/?$/.exec(proxy.trim())?.[1] ?? 0);
  const routes: Record<number,string> = {31001: '203.0.113.11', 31002: '203.0.113.27', 31003: '203.0.113.43', 31004: '203.0.113.59', 31005: '203.0.113.75', 31006: '203.0.113.91', 31007: '203.0.113.107', 31008: '203.0.113.123', 31010: '192.0.2.29', 31009: '203.0.113.139', 31101: '203.0.113.11', 31103: '203.0.113.43', 31110: '203.0.113.155'};
  return routes[port] ?? '198.51.100.' + (20 + port % 210);
}

function validate(input: AccountInput) {
  if (!input.name.trim()) throw '请填写名称';
  validateProxy(input.proxy);
  if (input.expectedIp.trim() && !/^(\d{1,3}\.){3}\d{1,3}$|:/.test(input.expectedIp.trim())) throw '出口 IP 格式不对';
  if (demo_accounts.some((a) => a.name === input.name.trim() && a.id !== input.id)) throw '已有同名账号';
}

const demo: Api = {
  async listManagedWindows() {
    return DEMO_WINDOWS.map(w => ({...w, accountName:demoCurrentName(), status:w.hasBackgroundTasks?'shell':'idle', host:'演示终端', canFocus:false}));
  },
  async focusManagedWindow() { throw '浏览器演示没有连接电脑上的终端'; },
  async editCard(input) {
    const cards = [...demo_accounts, ...demo_ordinary];
    const a = cards.find(c => c.id === input.id);
    if (!a) throw '账号不存在';
    const pair = demo_ordinary.find(o => o.sourceTokenId === a.id) ?? ('sourceTokenId' in a ? demo_accounts.find(t => t.id === a.sourceTokenId) : undefined);
    if (a.expectedIp && a.expectedIp !== input.expectedIp) throw '固定出口不符';
    if (input.proxy !== a.proxy) {
      validateProxy(input.proxy);
      if (demoExit(input.proxy) !== input.expectedIp) throw '实测出口与固定出口不一致';
      if (pair && !input.updatePair) throw '请确认同时更新关联卡';
      const check = {ok:true, checkedAt:now(), ip:input.expectedIp};
      a.lastNetworkCheck = check;
      if (pair) { pair.proxy = input.proxy; pair.lastNetworkCheck = check; }
    }
    Object.assign(a, {name:input.name, note:input.note, plan:input.plan, proxy:input.proxy, expectedIp:input.expectedIp});
  },
  async getState() {
    await sleep(160);
    return {
      version: '0.2.2', dataRoot: DEMO_ROOT, defaultProxy: 'http://127.0.0.1:7897',
      accounts: demo_accounts.map((a) => ({ ...a })), normal: { ...demo_normal },
      ordinaryAccounts: demo_ordinary.map(publicOrdinary), conflicts: [],
    };
  },
  async addAccount(input) {
    validate(input);
    if (!/^sk-ant-oat01-/.test(input.token.trim())) throw '这不像 setup token（应以 sk-ant-oat01- 开头）';
    await sleep(1600);
    const ip = input.expectedIp.trim() || demoExit(input.proxy);
    const a = acct({
      name: input.name.trim(), plan: input.plan, proxy: input.proxy.trim(), expectedIp: ip, note: input.note.trim(),
      tokenTail: input.token.trim().slice(-6), addedAt: now(),
      lastNetworkCheck: { ok: true, checkedAt: now(), ip },
      lastUsageCheck: usage(0, [['5h', 0, 300], ['7d', 3, 9000]]),
    });
    demo_accounts = [...demo_accounts, a];
    return { account: { ...a }, replaced: false };
  },
  async updateAccount(input) {
    validate(input);
    await sleep(250);
    const old = demoFind(input.id);
    const routing = old.proxy !== input.proxy.trim() || old.expectedIp !== input.expectedIp.trim();
    const next: Account = {
      ...old, name: input.name.trim(), plan: input.plan, proxy: input.proxy.trim(), expectedIp: input.expectedIp.trim() || old.expectedIp, note: input.note.trim(),
      lastNetworkCheck: routing ? null : old.lastNetworkCheck, lastUsageCheck: routing ? null : old.lastUsageCheck,
    };
    demo_accounts = demo_accounts.map((a) => (a.id === next.id ? next : a));
    return { ...next };
  },
  async replaceToken(id, token) {
    if (!/^sk-ant-oat01-/.test(token.trim())) throw '这不像 setup token（应以 sk-ant-oat01- 开头）';
    await sleep(1400);
    const a = demoFind(id);
    a.tokenTail = token.trim().slice(-6);
    a.lastUsageCheck = usage(0, [['5h', 20, 200], ['7d', 40, 5000]]);
    return { ...a };
  },
  async deleteAccount(id) {
    const ordinary = demo_ordinary.find(o => o.id === id);
    if (ordinary) {
      if (ordinary.current) throw '请先切换到其他账号';
      demo_ordinary = demo_ordinary.filter(o => o !== ordinary);
      return { fellBackToNormal:false };
    }
    await sleep(200);
    const a = demoFind(id);
    demo_accounts = demo_accounts.filter((x) => x.id !== id);
    if (a.current) demo_normal = { ...demo_normal, current: true };
    return { fellBackToNormal: a.current };
  },
  async switchTo(id) {
    const a = demoFind(id);
    await sleep(900);
    if (a.lastNetworkCheck && !a.lastNetworkCheck.ok) throw '出口不一致，未切换';
    if (a.lastUsageCheck?.state === 'invalid') throw '令牌无效，未切换';
    demo_accounts = demo_accounts.map((x) => ({ ...x, current: x.id === id }));
    demo_normal = { ...demo_normal, current: false };
    return { switchedAt: now() };
  },
  async useNormal() {
    await sleep(600);
    demo_accounts = demo_accounts.map((x) => ({ ...x, current: false }));
    demo_normal = { ...demo_normal, current: true };
    return { switchedAt: now() };
  },
  async previewSwitch(id) {
    await sleep(700);
    const target = id === null ? null : demo_accounts.find((x) => x.id === id) ?? null;
    if (id !== null && !target) demoTarget(id);
    const from = demoCurrentName();
    const blockers: string[] = [];
    if (target?.lastNetworkCheck && !target.lastNetworkCheck.ok) blockers.push('目标号的出口不一致');
    if (target?.lastUsageCheck?.state === 'invalid') blockers.push('目标号的令牌无效');
    if (target?.name === '主力号') blockers.push('有个后台对话不是换号台开的（PID 11208），先在那个窗口退出');
    const planId = hex(16);
    demo_plans.set(planId, { target: id, at: Date.now() });
    return {
      planId, createdAt: now(), supported: true, blockers,
      windows: DEMO_WINDOWS.map((w) => ({ ...w, accountName: from })),
    };
  },
  async refreshSwitch(planId) {
    const saved = demo_plans.get(planId);
    if (!saved) throw '这次预览已失效，请重新检查';
    const plan = await demo.previewSwitch(saved.target);
    demo_plans.delete(plan.planId);
    demo_plans.set(planId, { ...saved, at: Date.now() });
    return { ...plan, planId };
  },
  async executeSwitch(planId) {
    const plan = demo_plans.get(planId);
    if (!plan) throw '这次预览已失效，请重新检查';
    if (Date.now() - plan.at > 5 * 60_000) throw '预览超过 5 分钟，窗口可能变了，请重新检查';
    demo_plans.delete(planId);
    const total = DEMO_WINDOWS.length;
    const steps: [string, string, number, number][] = [
      ['stopping', '正在退出窗口', 0, total],
      ['stopping', '正在退出窗口', 2, total],
      ['stopping', '正在停后台任务', total, total],
      ['switching', '正在换号', 0, 1],
      ['restoring', '正在恢复对话', 1, total],
      ['restoring', '正在恢复对话', 3, total],
    ];
    for (const [stage, text, completed, n] of steps) {
      demo_progress.set(planId, { stage, text, completed, total: n });
      await sleep(650);
    }
    demo_accounts = demo_accounts.map((x) => ({ ...x, current: x.id === plan.target }));
    demo_ordinary = demo_ordinary.map((x) => ({ ...x, current: x.id === plan.target }));
    demo_normal = { ...demo_normal, current: plan.target === null };
    const operationId = hex(16);
    const w = DEMO_WINDOWS[2];
    const result: SwitchResult = {
      operationId, switched: true, switchedAt: now(), restored: total - 1, errors: [],
      failed: [{ windowId: w.windowId, cwd: w.cwd, sessionId: w.sessionId, error: '终端窗口已被关闭，没法回到原窗口' }],
    };
    demo_ops.set(operationId, { result, attempts: {} });
    return copyResult(result);
  },
  async getSwitchProgress(planId) {
    await sleep(60);
    return demo_progress.get(planId) ?? { stage: 'preparing', text: '正在准备', completed: 0, total: 0 };
  },
  async retryRestore(operationId, windowId) {
    const op = demo_ops.get(operationId);
    if (!op) throw '找不到这次切换';
    await sleep(1200);
    const n = (op.attempts[windowId] = (op.attempts[windowId] ?? 0) + 1);
    if (op.result.failed.some((f) => f.windowId === windowId)) {
      // 第一次重试故意失败，第二次成功：两条路都能在演示里看到
      op.result = n === 1
        ? { ...op.result, failed: op.result.failed.map((f) => (f.windowId === windowId ? { ...f, error: '原终端还在关闭，请稍后再试' } : f)) }
        : { ...op.result, restored: op.result.restored + 1, failed: op.result.failed.filter((f) => f.windowId !== windowId) };
    }
    return copyResult(op.result);
  },
  async checkUsage(id) {
    const o = demo_ordinary.find((x) => x.id === id);
    if (o) {
      await sleep(1100);
      const old = o.lastUsageCheck?.buckets ?? [['5h', 0, 300], ['7d', 0, 9000]].map(([key, u, r]) => ({ key: key as string, utilization: u as number, resetsAt: later(r as number), status: 'allowed' }));
      const buckets = old.map((b) => ({ ...b, utilization: Math.min(100, b.utilization + Math.round(Math.random() * 3)) }));
      o.lastUsageCheck = { ok: true, checkedAt: now(), state: 'ok', buckets, source: 'native-probe' };
      return o.lastUsageCheck;
    }
    const a = demoFind(id);
    await sleep(1100 + Math.random() * 600);
    if (a.lastNetworkCheck && !a.lastNetworkCheck.ok) throw '出口不一致，未查询';
    if (a.lastUsageCheck?.state === 'invalid') throw '令牌无效，请重新生成';
    const old = a.lastUsageCheck?.buckets ?? [];
    const buckets = old.map((b) => {
      const passed = b.resetsAt && Date.parse(b.resetsAt) <= Date.now();
      const u = passed ? Math.round(Math.random() * 6) : Math.min(100, b.utilization + (b.utilization >= 100 ? 0 : Math.round(Math.random() * 2)));
      return { ...b, utilization: u, resetsAt: passed ? later(b.key === '5h' ? 300 : 10080) : b.resetsAt, status: u >= 100 ? 'rejected' : 'allowed' };
    });
    a.lastUsageCheck = {
      ok: true, checkedAt: now(), buckets, source: 'native-probe',
      state: buckets.some((b) => (b.key === '5h' || b.key === '7d') && b.utilization >= 100) ? 'limited' : 'ok',
    };
    a.lastNetworkCheck = { ok: true, checkedAt: now(), ip: a.expectedIp };
    return a.lastUsageCheck;
  },
  async beginNormalLogin({ name, proxy, expectedIp, sourceTokenId, note, plan }) {
    await sleep(500);
    if (!name.trim()) throw '请填写名称';
    validateProxy(proxy);
    const ip = expectedIp.trim();
    if (!/^(\d{1,3}\.){3}\d{1,3}$|:/.test(ip)) throw '请先测出这个号的固定出口';
    const p = proxy.trim().replace(/\/$/, '');
    // 和真后端一样：原卡模式要原卡有组织编号、端口和 IP 都沿用原卡；其他卡和普通账号一律不能撞
    const source = sourceTokenId ? demo_accounts.find((a) => a.id === sourceTokenId) : undefined;
    if (sourceTokenId) {
      if (!source) throw '找不到这张令牌卡';
      if (!source.org) throw `「${source.name}」没有组织编号，暂不能核实是同一个号`;
      if (!samePort(source.proxy, p) || !sameIp(source.expectedIp, ip)) throw `端口和出口要和「${source.name}」一样`;
      const linked = demo_ordinary.find((o) => o.org === source.org);
      if (linked) throw `这个号已有普通账号「${linked.name}」`;
    }
    for (const a of demo_accounts) {
      if (a.id === source?.id) continue;
      if (samePort(a.proxy, p) || sameIp(a.expectedIp, ip)) throw `端口或出口已是令牌卡「${a.name}」的；同一个人请选「已有令牌卡」`;
    }
    for (const o of demo_ordinary) {
      if (samePort(o.proxy, p) || sameIp(o.expectedIp, ip)) throw `端口或出口已是「${o.name}」的，每个号要单独的出口`;
    }
    const loginId = hex(16);
    demo_logins.set(loginId, { name: name.trim(), proxy: p, expectedIp: ip, at: Date.now(), cancelled: false, source, note, plan });
    return { loginId };
  },
  async getNormalLogin(loginId) {
    await sleep(120);
    const l = demo_logins.get(loginId);
    if (!l) throw '这次登记已失效';
    if (l.done) return { status: 'complete' };
    if (l.cancelled) return { status: 'failed', error: '已取消登记' };
    const elapsed = Date.now() - l.at;
    // 演示：约 1 秒后有登录链接，约 7 秒后当作用户已在浏览器里授权
    const loginUrl = elapsed < 1000 ? undefined : `https://claude.ai/oauth/authorize?code=true&client_id=demo&response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A54545%2Fcallback&state=demo-${loginId.slice(0, 8)}`;
    // 演示：名字里带「失败」就模拟浏览器登错号；原卡模式名字里带「别人」就模拟登的不是原卡那个号
    if (l.name.includes('失败')) return elapsed < 4000 ? { status: 'waiting', loginUrl } : { status: 'failed', error: '登录的号和这个出口记下的号对不上，未登记' };
    if (l.source && l.name.includes('别人')) return elapsed < 4000 ? { status: 'waiting', loginUrl } : { status: 'failed', error: `登录的不是「${l.source.name}」那个号，未登记` };
    if (elapsed < 7000) return { status: 'waiting', loginUrl };
    demoCommit(l);
    return { status: 'complete' };
  },
  async cancelNormalLogin(loginId) {
    await sleep(150);
    const l = demo_logins.get(loginId);
    if (!l || l.done) return;
    // 演示：名字里带「撞车」就模拟授权刚好完成、取消落空（真后端这时取消也回成功）
    if (l.name.includes('撞车')) return demoCommit(l);
    l.cancelled = true;
  },
  async checkNormalUsage() {
    await sleep(900);
    const old = demo_normal.lastUsageCheck?.buckets ?? [];
    const buckets = old.map((b) => ({ ...b, utilization: Math.min(100, b.utilization + Math.round(Math.random() * 2)) }));
    demo_normal = {
      ...demo_normal,
      lastUsageCheck: { ok: true, checkedAt: now(), state: 'ok', buckets, source: 'native-probe', overage: { status: 'rejected', disabledReason: null } },
    };
    return demo_normal.lastUsageCheck!;
  },
  async detectExit(proxy) {
    validateProxy(proxy);
    await sleep(700);
    return { ip: demoExit(proxy) };
  },
  async checkNetwork(id) {
    const a = demo_ordinary.find(o => o.id === id) ?? demoFind(id);
    await sleep(900);
    const observed = demoExit(a.proxy ?? '');
    a.lastNetworkCheck = observed === a.expectedIp
      ? { ok: true, checkedAt: now(), ip: observed }
      : { ok: false, checkedAt: now(), ip: observed, error: '实测出口与固定出口不一致' };
    if (!a.lastNetworkCheck.ok) throw a.lastNetworkCheck.error;
    return a.lastNetworkCheck;
  },
  async exportCpa() {
    throw '演示模式不能导出';
  },
  async openDataFolder() {
    throw '演示模式没有数据目录';
  },
  async openHelp() {
    throw '演示模式不打开说明';
  },
};

export const api: Api = isTauri ? real : demo;
