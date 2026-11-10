import { useEffect, useRef, useState } from 'react';
import { api, errorText, type Account, type AppState } from './api';
import { Shell } from './Dialogs';
import { IconChevron, IconCopy } from './icons';
import { portOf, sameIp, samePort } from './util';

// 添加普通账号：先定好这个号专用的固定出口，后端跑原生官方登录并给出登录链接（回调到本机）。
// 用户复制链接，到走这个出口的浏览器里授权；授权后自动完成。登记只是记下这个号，不切换当前号。
// 已有令牌卡的同一个个人账号：沿用原卡记下的端口和 IP（锁住不能改），两张卡都保留；
// 后端登录后核对同组织、个人订阅、同端口同 IP。浏览器和节点由用户自己操作，核对浏览器出口只是可选帮助。

const LOOPBACK = /^http:\/\/(127\.0\.0\.1|localhost|\[::1\]):\d+\/?$/;
const IPV4 = /^(\d{1,3}\.){3}\d{1,3}$/;
const TRACE = ['https://claude.com/cdn-cgi/trace', 'https://claude.ai/cdn-cgi/trace'];
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

type Mode = 'new' | 'token';
interface ExitCheck {
  error?: string;
  warn?: string;
  /** 撞到的令牌卡：新号模式下可以改选它当原卡 */
  card?: Account;
}

/** 令牌卡能不能当「同一个号」的原卡：要有组织编号，且还没有对应的普通账号 */
function sourceState(state: AppState, a: Account): { ok: boolean; reason?: string } {
  if (!a.org) return { ok: false, reason: '没有组织编号，暂不能核实' };
  const linked = (state.ordinaryAccounts ?? []).find((o) => o.org && o.org === a.org);
  if (linked) return { ok: false, reason: `已有普通账号「${linked.name}」` };
  return { ok: true };
}

/** 出口必须各号独立：和别的令牌卡、普通账号撞了直接拦；和本机账号登录重复只提醒（可能就是同一个号） */
function exitCheck(state: AppState, proxy: string, ip: string, sourceId: string | null): ExitCheck {
  const p = proxy.trim();
  const i = ip.trim();
  for (const a of state.accounts) {
    if (a.id === sourceId) continue;
    if (p && samePort(a.proxy, p)) return { error: `端口已是令牌卡「${a.name}」的`, card: a };
    if (i && sameIp(a.expectedIp, i)) return { error: `出口 ${i} 已是令牌卡「${a.name}」的`, card: a };
  }
  // 后端不给普通账号的代理，端口重复由后端拦
  for (const o of state.ordinaryAccounts ?? []) {
    if (i && sameIp(o.expectedIp, i)) return { error: `出口 ${i} 已是「${o.name}」的` };
  }
  if (state.normal.email && ((p && samePort(state.defaultProxy, p)) || (i && sameIp(state.normal.expectedIp, i)))) {
    return { warn: '本机账号登录也走这个出口，只能登记同一个号' };
  }
  return {};
}

/** 可选帮助：用这个端口单独开一个 Edge（独立资料夹，不碰默认窗口），只打开两个核对页，不带登录链接 */
function edgeCommand(proxy: string): string {
  const p = proxy.trim().replace(/\/$/, '');
  const port = portOf(p);
  if (!LOOPBACK.test(proxy.trim()) || port === null) return '';
  return `$d = Join-Path $env:LOCALAPPDATA 'ClaudeAccountManager-browser\\port-${port}'; `
    + `Start-Process msedge -ArgumentList "--user-data-dir=\`"$d\`"", '--proxy-server=${p}', '--no-first-run', '--no-default-browser-check', '${TRACE[0]}', '${TRACE[1]}'`;
}

export function NormalLoginDialog({ state, initial, autoBegin, onBack, onClose, onDone }: {
  state: AppState;
  initial?: {name: string; note: string; plan: import("./api").Plan; proxy:string; expectedIp:string; sourceTokenId?:string};
  autoBegin?:boolean;
  onBack?():void;
  onClose(): void;
  /** 登记完成：刷新界面 */
  onDone(msg: string): void;
}) {
  const [mode, setMode] = useState<Mode>(initial?.sourceTokenId ? 'token' : 'new');
  const [sourceId, setSourceId] = useState<string | null>(initial?.sourceTokenId ?? null);
  const [name, setName] = useState(initial?.name ?? '');
  const [proxy, setProxy] = useState(initial?.proxy ?? '');
  const [ip, setIp] = useState(initial?.expectedIp ?? '');
  const [detected, setDetected] = useState('');
  const [detecting, setDetecting] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState('');
  const [loginId, setLoginId] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [loginUrl, setLoginUrl] = useState('');
  const [copied, setCopied] = useState('');
  const [helpOpen, setHelpOpen] = useState(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  // 窗口被关掉时还在等登录：尽量通知后端取消，免得之后悄悄登记
  const pending = useRef<string | null>(null);
  pending.current = loginId;
  useEffect(() => () => {
    if (pending.current) api.cancelNormalLogin(pending.current).catch(() => {});
  }, []);
  // 测出口时又改了代理，旧结果作废
  const proxyNow = useRef(proxy);
  proxyNow.current = proxy;
  // 取消时暂停轮询；完成只报一次（轮询和取消后的回查都可能先看到完成）
  const cancelRef = useRef(false);
  const finished = useRef(false);

  const cards = state.accounts;
  const source = mode === 'token' ? cards.find((a) => a.id === sourceId) ?? null : null;
  const check = exitCheck(state, proxy, ip, source?.id ?? null);
  const ipValid = IPV4.test(ip.trim()) || ip.includes(':');
  const mismatch = !!source && !!detected && !sameIp(detected, ip);
  const hit = !source && check.card ? { card: check.card, ...sourceState(state, check.card) } : null;
  const edge = edgeCommand(proxy);
  const waiting = loginId !== null;

  function setRoute(nextProxy: string, nextIp: string) {
    setProxy(nextProxy);
    setIp(nextIp);
    setDetected('');
    setError('');
  }
  function pickMode(m: Mode) {
    if (m === mode) return;
    setMode(m);
    setSourceId(null);
    setRoute('', '');
  }
  function pickSource(a: Account) {
    setMode('token');
    setSourceId(a.id);
    setRoute(a.proxy, a.expectedIp);
    if (!name.trim()) setName(a.name);
  }

  async function detect() {
    const p = proxy.trim();
    if (!LOOPBACK.test(p)) return setError('只支持本机 HTTP 代理');
    setDetecting(true);
    setError('');
    try {
      const r = await api.detectExit(p);
      if (!alive.current || proxyNow.current.trim() !== p) return;
      // 原卡模式 IP 锁住，只拿来对照；新号模式填进出口 IP
      if (!source) setIp(r.ip);
      setDetected(r.ip);
    } catch (err) {
      if (alive.current) setError(errorText(err));
    } finally {
      if (alive.current) setDetecting(false);
    }
  }

  async function start(e?: React.FormEvent) {
    e?.preventDefault();
    if (waiting || starting) return;
    if (mode === 'token') {
      if (!source) return setError('先选这个号原来的令牌卡');
      const s = sourceState(state, source);
      if (!s.ok) return setError(`「${source.name}」${s.reason}`);
    }
    if (!name.trim()) return setError('请填写名称');
    if (!LOOPBACK.test(proxy.trim())) return setError('只支持本机 HTTP 代理');
    if (!ipValid) return setError('先测一下这个号的固定出口');
    if (check.error) return setError(check.error);
    if (mismatch) return setError(`测到的出口 ${detected} 和原卡记下的不一样`);
    setStarting(true);
    setError('');
    try {
      const r = await api.beginNormalLogin({
        name: name.trim(), proxy: proxy.trim(), expectedIp: ip.trim(),
        note: initial?.note, plan: initial?.plan,
        ...(source ? { sourceTokenId: source.id } : {}),
      });
      if (!alive.current) {
        api.cancelNormalLogin(r.loginId).catch(() => {});
        return;
      }
      setLoginId(r.loginId);
    } catch (err) {
      if (alive.current) setError(errorText(err));
    } finally {
      if (alive.current) setStarting(false);
    }
  }

  const begun = useRef(false);
  useEffect(() => {
    if (autoBegin && !begun.current) { begun.current = true; void start(); }
  }, [autoBegin]);
  function complete() {
    if (finished.current) return;
    finished.current = true;
    pending.current = null;
    setLoginId(null);
    onDone(`已添加「${name.trim()}」，没有切换当前号`);
  }

  // 等登录：每 1.5 秒问一次后端；读不到状态不算失败，继续等，用户可以取消
  useEffect(() => {
    if (!loginId) return;
    let stop = false;
    const tick = async () => {
      if (cancelRef.current) return;
      try {
        const r = await api.getNormalLogin(loginId);
        if (stop || !alive.current || cancelRef.current) return;
        if (r.loginUrl) setLoginUrl(r.loginUrl);
        if (r.status === 'complete') {
          stop = true;
          complete();
        } else if (r.status === 'failed') {
          stop = true;
          pending.current = null;
          setLoginId(null);
          setLoginUrl('');
          setError(r.error ?? '登录没有完成，未登记');
        }
      } catch (err) {
        if (!stop && alive.current && !cancelRef.current) setError(errorText(err));
      }
    };
    const t = setInterval(tick, 1500);
    tick();
    return () => { stop = true; clearInterval(t); };
    // name 只用于提示文案，登记开始后不能再改
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loginId]);

  async function copy(text: string) {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(text);
      setTimeout(() => { if (alive.current) setCopied((c) => (c === text ? '' : c)); }, 1500);
    } catch {
      setError('复制失败，请手动选中复制');
    }
  }

  async function cancel() {
    if (!loginId) return onClose();
    const id = loginId;
    cancelRef.current = true;
    setCancelling(true);
    setError('');
    try {
      await api.cancelNormalLogin(id);
      pending.current = null;
      // 授权刚好完成时取消会落空（后端照样回成功）：再查几次，已登记就按完成处理
      for (let n = 0; n < 10 && alive.current; n++) {
        const r = await api.getNormalLogin(id).catch(() => null);
        if (!alive.current) return;
        if (r?.status === 'complete') return complete();
        if (r?.status === 'failed') break;
        await wait(300);
      }
      if (alive.current) onClose();
    } catch (err) {
      cancelRef.current = false;
      if (alive.current) setError(errorText(err));
    } finally {
      if (alive.current) setCancelling(false);
    }
  }

  let note: { cls: string; text: string };
  if (check.error) note = { cls: 'bad', text: check.error };
  else if (mismatch) note = { cls: 'bad', text: `测到 ${detected}，和原卡不一样，先别登录` };
  else if (source) note = detected ? { cls: 'ok', text: `测到 ${detected}，和原卡一致` } : { cls: '', text: `沿用「${source.name}」的端口和出口` };
  else if (mode === 'token') note = { cls: '', text: '先在上面选原来的令牌卡' };
  else note = detected ? { cls: 'ok', text: `测到 ${detected}，以后出口变了会拦下` } : { cls: '', text: '每个号要单独的出口，换了 IP 会被拦下' };
  const locked = mode === 'token';

  return (
    <Shell
      title="账号登录授权"
      sub={waiting ? '在走这个出口的浏览器里授权' : '每个号一次官方登录，不切换当前号'}
      busy={waiting || starting}
      onClose={onClose}
      onSubmit={start}
      width={560}
      foot={
        <>
          <span className="form-error" role="alert" title={error}>{error}</span>
          {waiting ? (
            <>
              <button className="btn-text" type="button" onClick={cancel} disabled={cancelling}>
                {cancelling && <span className="spinner" />}取消登记
              </button>
              <button className="btn btn-white" type="submit" disabled><span className="spinner" />等待授权</button>
            </>
          ) : (
            <>
              <button className="btn-text" type="button" onClick={onBack ?? onClose} disabled={starting}>{onBack ? '返回编辑' : '取消'}</button>
              <button className="btn btn-white" type="submit" disabled={starting || !!check.error || mismatch || (locked && !source)}>
                {starting && <span className="spinner" />}开始登录
              </button>
            </>
          )}
        </>
      }
    >
      {waiting ? (
        <>
          <ol className="guide login-steps">
            <li><span>复制下面的<b>官方登录链接</b></span></li>
            <li><span>在<b>走 {ip.trim()} 出口</b>的浏览器里打开</span></li>
            <li><span>登录<b>{source ? `「${source.name}」那个号` : '要添加的那个号'}</b>并授权</span></li>
          </ol>
          <label className="field">
            <span>官方登录链接<em>授权后回到本机，自动完成</em></span>
            <div className="input-icon input-action">
              <input className="mono" value={loginUrl || '正在生成…'} readOnly onFocus={(e) => e.target.select()} spellCheck={false} />
              <button type="button" className="in-btn" onClick={() => copy(loginUrl)} disabled={!loginUrl}>
                {loginUrl && copied === loginUrl ? '已复制' : '复制'}
              </button>
            </div>
          </label>
          <p className="flow-foot-note">不用在这里填授权码；中途可以取消</p>
        </>
      ) : (
        <>
          <div className="label">账号</div>
          <div className="card">
            <div className="seg seg-2 mode-seg" role="radiogroup" aria-label="账号来源">
              <button type="button" role="radio" aria-checked={mode === 'new'} onClick={() => pickMode('new')}>新账号</button>
              <button type="button" role="radio" aria-checked={mode === 'token'} onClick={() => pickMode('token')} disabled={cards.length === 0} title={cards.length === 0 ? '还没有令牌卡' : undefined}>
                已有令牌卡的同一个号
              </button>
            </div>
            {mode === 'token' && (
              <>
                <div className="src-list" role="radiogroup" aria-label="原令牌卡">
                  {cards.map((a) => {
                    const s = sourceState(state, a);
                    return (
                      <button key={a.id} type="button" role="radio" className="src-row" aria-checked={sourceId === a.id} disabled={!s.ok} onClick={() => pickSource(a)}>
                        <span className="src-name">{a.name}</span>
                        <span className="src-route mono">:{portOf(a.proxy) ?? '?'} · {a.expectedIp || '未测出口'}</span>
                        {!s.ok && <span className="src-why">{s.reason}</span>}
                      </button>
                    );
                  })}
                </div>
                <p className="field-note src-note">只限个人订阅，登录后核对组织</p>
              </>
            )}
            <label className="field">
              <span>名称</span>
              <input value={name} onChange={(e) => { setName(e.target.value); setError(''); }} maxLength={60} placeholder="填这个号的邮箱，方便区分" autoFocus />
            </label>
          </div>

          <div className="label">固定出口</div>
          <div className="card">
            <div className="row-2">
              <label className="field">
                <span>本机代理<em>{locked ? '沿用原卡' : '这个号专用'}</em></span>
                <input className={`mono${locked ? ' locked' : ''}`} value={proxy} readOnly={locked} onChange={(e) => setRoute(e.target.value, ip)} placeholder={locked ? '先选原卡' : 'http://127.0.0.1:1808x'} spellCheck={false} />
              </label>
              <label className="field">
                <span>出口 IP{locked && <em>沿用原卡</em>}</span>
                <div className="input-icon input-action">
                  <input className={`mono${locked ? ' locked' : ''}`} value={ip} readOnly={locked} onChange={(e) => setRoute(proxy, e.target.value)} placeholder={locked ? '先选原卡' : '先测一下'} spellCheck={false} />
                  <button type="button" className="in-btn" onClick={detect} disabled={detecting || !proxy.trim()}>
                    {detecting && <span className="spinner" />}测一下
                  </button>
                </div>
              </label>
            </div>
            <p className={`field-note${note.cls ? ` ${note.cls}` : ''}`}>
              {note.text}
              {hit && (hit.ok
                ? <button type="button" className="note-link" onClick={() => pickSource(hit.card)}>是同一个人？改选原卡</button>
                : <span className="note-sub">那张卡{hit.reason}</span>)}
            </p>
          </div>

          <button type="button" className="help-toggle" aria-expanded={helpOpen} onClick={() => setHelpOpen((v) => !v)}>
            <IconChevron />核对浏览器出口<em>可选</em>
          </button>
          {helpOpen && (
            <div className="card browser-help">
              <p className="bh-lead">「测一下」只测端口，<b>不代表浏览器</b></p>
              <p className="field-note">在授权用的窗口打开，<code>ip=</code> 应是 <b className="mono">{ipValid ? ip.trim() : '这个号的出口'}</b></p>
              <div className="bh-urls">
                {TRACE.map((u) => (
                  <button key={u} type="button" className="cmd" onClick={() => copy(u)} title="复制">
                    <code>{u}</code>{copied === u ? <span className="ok">已复制</span> : <IconCopy />}
                  </button>
                ))}
              </div>
              <div className="bh-edge">
                <button type="button" className="cmd" onClick={() => copy(edge)} disabled={!edge} title={edge || '先填本机代理'}>
                  <span className="cmd-label">复制独立 Edge 命令</span>{edge && copied === edge ? <span className="ok">已复制</span> : <IconCopy />}
                </button>
                <em>在 PowerShell 运行，只开核对页</em>
              </div>
            </div>
          )}

          <ul className="tips">
            <li>代理要这个号专用，<b>别在同一端口切节点</b></li>
            <li>浏览器也要<b>走这个出口</b>再授权</li>
            {source && <li>两张卡都保留，<b>出口不变</b></li>}
            <li>登记完成前<b>不会切换</b>当前号</li>
            {check.warn && <li className="warn">{check.warn}</li>}
          </ul>
        </>
      )}
    </Shell>
  );
}
