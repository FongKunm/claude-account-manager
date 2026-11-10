import { QuotaIcon } from "./QuotaIcon";
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  api, errorText,
  type RestorePermission, type SwitchFailure, type SwitchPlan, type SwitchProgress, type SwitchResult, type SwitchWindow,
} from './api';
import { Shell } from './Dialogs';
import { IconAlert } from './icons';

// 切换（Windows）：先列出会退出的窗口 → 确认 → 退出、换号、各自回到原对话。
// 预览阶段不停任何东西；「以后不再提示」不跳过这一步。

type Phase =
  | { kind: 'loading' }
  | { kind: 'preview'; plan: SwitchPlan }
  | { kind: 'running'; plan: SwitchPlan }
  | { kind: 'done'; plan: SwitchPlan; result: SwitchResult };

const PERMS: { value: RestorePermission; label: string }[] = [
  { value: 'preserve', label: '保持原样' },
  { value: 'default', label: '标准权限' },
  { value: 'bypassPermissions', label: '完全权限' },
];

const MODE_TEXT: Record<string, string> = {
  bypassPermissions: '完全权限',
  acceptEdits: '自动改文件',
  plan: '计划模式',
  auto: '自动模式',
};

function baseName(path: string): string {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function WindowRow({ w, pending, onLocate }: { w: SwitchWindow; pending: string; onLocate(w: SwitchWindow): void }) {
  const mode = MODE_TEXT[w.permissionMode];
  const reasons = w.blockers ?? [];
  return <li><span className="term-symbol"><QuotaIcon name="terminal" size={18}/></span>
    <div className="term-id"><strong>{w.name || baseName(w.cwd)}</strong><small>{w.kind === 'background' ? '后台会话' : '终端'} · {w.accountName} · <span className="sid">{w.sessionId.slice(0,8)}</span>{mode && ' · '+mode}</small><small className="cwd">{w.cwd}</small>
      {w.model && <small className="restore-model">恢复设置：{w.model}{w.effort ? ' · '+w.effort : ''}</small>}
      {reasons.map((reason, i) => <small className="terminal-blocker" key={i}><IconAlert />{reason}</small>)}
    </div>
    <div className="term-state"><span className={'ui-tag'+(reasons.length || w.hasBackgroundTasks?' amber':' good')}>{reasons.length?'待处理':w.hasBackgroundTasks?'将停止后台任务':'已就绪'}</span></div>
    <button type="button" className="ui-btn minor" disabled={!w.canFocus || !!pending} title={w.canFocus?'显示这个终端或标签页':'该终端当前不能定位'} onClick={()=>onLocate(w)}>{pending===w.windowId?'正在定位…':'定位窗口'} <QuotaIcon name="external" size={13}/></button>
  </li>;
}

function SwitchTimeline({ count, progress }: { count: number; progress: SwitchProgress | null }) {
  const stages = [
    ['checking', '核对目标账号与固定出口'],
    ...(count ? [['stopping', '依次退出 '+count+' 个旧窗口，确认后台任务已停止']] : []),
    ['switching', '切换账号与专用代理'],
    ...(count ? [['restoring', '恢复各自的对话，并核对原模型和思考强度']] : []),
  ];
  const active = progress ? stages.findIndex(([key]) => key === progress.stage) : -1;
  const detail = progress && progress.total > 0
    ? progress.text+' · '+progress.completed+'/'+progress.total : progress?.text;
  if (progress?.text.startsWith('未切换')) return <p role="status">{detail}</p>;
  return <div className="switch-timeline" role="status" aria-live="polite">
    {stages.map(([key, text], n) => <div key={key} className={n < active ? 'done' : n === active ? 'now' : ''}>
      <span>{n < active ? <QuotaIcon name="check" size={13}/> : n+1}</span>{text}
    </div>)}
    {detail && <small className="helper">{detail}</small>}
  </div>;
}

function FailureRow({ f, busy, spinning, onRetry, onLocate }: { f: SwitchFailure; busy: boolean; spinning: boolean; onRetry(): void; onLocate?: () => void }) {
  return (
    <li className="win-row bad">
      <div className="win-main">
        <span className="win-name" title={f.cwd}>{baseName(f.cwd)}</span>
        <span className="win-sid mono" title={f.sessionId}>{f.sessionId.slice(0, 8)}</span>
        {onLocate && <button className="btn btn-sm" type="button" onClick={onLocate} disabled={busy}>定位窗口</button>}
        <button className="btn btn-sm" type="button" onClick={onRetry} disabled={busy}>
          {spinning && <span className="spinner" />}重试恢复
        </button>
      </div>
      <div className="win-err" title={f.error}>{f.error}</div>
    </li>
  );
}

export function SwitchFlow({ target, from, onUnsupported, onClose, onSwitched }: {
  /** id 为 null 是切回本机账号登录 */
  target: { id: string | null; name: string; ip?: string };
  from?: {name: string; ip: string} | null;
  /** 这台电脑不支持（Mac）：交回原来的切换 */
  onUnsupported(): void;
  onClose(): void;
  /** 账号已换：刷新界面 */
  onSwitched(result: SwitchResult): void;
}) {
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [error, setError] = useState('');
  const [perm, setPerm] = useState<RestorePermission>('preserve');
  const [progress, setProgress] = useState<SwitchProgress | null>(null);
  const [retrying, setRetrying] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [checkError, setCheckError] = useState('');
  const [locating, setLocating] = useState('');
  const [toast, setToast] = useState('');
  const refreshNow = useRef<(() => void) | null>(null);
  // 严格模式开发时副作用会「装上→卸下→再装上」：装上时必须显式设回 true
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  // 父组件每次刷新都会给新函数；放进 ref，免得重新预览把进行中的切换打回去
  const unsupported = useRef(onUnsupported);
  unsupported.current = onUnsupported;
  // 每次预览、执行都换新编号；晚到的旧预览（编号不是最新）直接丢掉，不能盖掉新状态
  const seq = useRef(0);
  // 同一帧里连按两次回车/连点，state 还没更新，用 ref 挡住第二次
  const executing = useRef(false);
  const retryLock = useRef(false);

  const preview = useCallback(async () => {
    const my = ++seq.current;
    setPhase({ kind: 'loading' });
    setError('');
    setCheckError('');
    try {
      const plan = await api.previewSwitch(target.id);
      if (!alive.current || my !== seq.current) return;
      if (!plan.supported) return unsupported.current();
      setPhase({ kind: 'preview', plan });
    } catch (err) {
      if (alive.current && my === seq.current) setError(errorText(err));
    }
  }, [target.id]);

  useEffect(() => { preview(); }, [preview]);

  const previewId = phase.kind === 'preview' ? phase.plan.planId : null;
  useEffect(() => {
    if (!previewId) return;
    let stopped = false;
    let inFlight = false;
    let timer: ReturnType<typeof setTimeout>;
    const tick = async () => {
      if (stopped || inFlight || executing.current) return;
      clearTimeout(timer);
      inFlight = true;
      const my = seq.current;
      setChecking(true);
      try {
        const plan = await api.refreshSwitch(previewId);
        if (stopped || !alive.current || my !== seq.current || executing.current) return;
        setCheckError('');
        setPhase(p => p.kind === 'preview' && p.plan.planId === previewId ? { kind:'preview', plan } : p);
      } catch (err) {
        if (!stopped && alive.current && my === seq.current) setCheckError(errorText(err));
      } finally {
        inFlight = false;
        if (!stopped && alive.current) {
          setChecking(false);
          timer = setTimeout(tick, 2000);
        }
      }
    };
    const focus = () => { void tick(); };
    refreshNow.current = focus;
    timer = setTimeout(tick, 2000);
    window.addEventListener('focus', focus);
    return () => { stopped = true; clearTimeout(timer); window.removeEventListener('focus', focus); refreshNow.current = null; };
  }, [previewId]);

  useEffect(() => {
    if (!toast) return;
    let timer: ReturnType<typeof setTimeout>;
    const pause = () => clearTimeout(timer);
    const show = () => { pause(); if (document.hasFocus()) timer = setTimeout(()=>setToast(''), 3000); };
    show(); window.addEventListener('focus', show); window.addEventListener('blur', pause);
    return () => { pause(); window.removeEventListener('focus', show); window.removeEventListener('blur', pause); };
  }, [toast]);

  async function locate(w: SwitchWindow) {
    setLocating(w.windowId); setError(''); setToast('');
    try { await api.focusManagedWindow(w.windowId); if (alive.current) setToast('已定位到「'+(w.name || baseName(w.cwd))+'」'); }
    catch (err) { if (alive.current) setError(errorText(err)); }
    finally { if (alive.current) setLocating(''); }
  }

  // 执行中轮询进度（只读）
  const running = phase.kind === 'running' ? phase.plan.planId : null;
  useEffect(() => {
    if (!running) return;
    let stop = false;
    const tick = async () => {
      try {
        const p = await api.getSwitchProgress(running);
        if (!stop) setProgress(p);
      } catch { /* 进度读不到不影响切换本身 */ }
    };
    tick();
    const t = setInterval(tick, 400);
    return () => { stop = true; clearInterval(t); };
  }, [running]);

  async function execute(plan: SwitchPlan) {
    if (executing.current || plan.blockers.length || checkError || locating) return;
    executing.current = true;
    seq.current++; // 作废还没回来的预览：切换开始后不能再被预览结果改回去
    setPhase({ kind: 'running', plan });
    setProgress(null);
    setError('');
    try {
      const result = await api.executeSwitch(plan.planId, perm);
      if (result.switched) onSwitched(result);
      if (alive.current) setPhase({ kind: 'done', plan, result });
    } catch (err) {
      // 保留原弹窗，自动刷新状态；后台仍会在执行前重新核对一次。
      executing.current = false;
      if (alive.current) {
        setPhase({ kind: 'preview', plan });
        setError(errorText(err));
        setCheckError('正在重新核对当前状态，请稍候。');
      }
    }
  }

  async function retry(operationId: string, windowId: string) {
    if (retryLock.current) return;
    retryLock.current = true;
    setRetrying(windowId);
    setError('');
    try {
      // 后端回的是这次切换的累计结果（已恢复数、剩下的失败、全部错误），直接采用
      const r = await api.retryRestore(operationId, windowId);
      if (!alive.current) return;
      setPhase((p) => (p.kind === 'done' ? { ...p, result: r } : p));
      const still = r.failed.find((f) => f.windowId === windowId);
      if (still) setError(`重试没成功：${still.error}`);
    } catch (err) {
      if (alive.current) setError(errorText(err));
    } finally {
      retryLock.current = false;
      if (alive.current) setRetrying(null);
    }
  }

  const title = phase.kind === 'running' ? '正在切换账号'
    : phase.kind === 'done' && phase.result.switched && !phase.result.failed.length && !phase.result.errors.length
      ? (phase.result.restored ? phase.result.restored+' 个原对话已恢复' : '账号已切换')
    : target.id === null ? '切回本机账号登录' : `切换到「${target.name}」`;
  const isRunning = phase.kind === 'running';
  const busy = isRunning || retrying !== null;

  let sub: string | undefined;
  let body: React.ReactNode;
  let primary: React.ReactNode;

  if (phase.kind === 'loading') {
    sub = error ? '需要重新检查' : '正在查看开着的窗口';
    body = error
      ? <div className="flow-empty bad">{error}</div>
      : <div className="flow-empty"><span className="spinner lg" /></div>;
    primary = error
      ? <button className="ui-btn primary" type="submit">重新检查</button>
      : <button className="ui-btn primary" type="submit" disabled><span className="spinner" />检查中</button>;
  } else if (phase.kind === 'running') {
    sub = target.name;
    body = <SwitchTimeline count={phase.plan.windows.length} progress={progress}/>;
  } else if (phase.kind === 'preview') {
    const { plan } = phase;
    const n = plan.windows.length;
    const blocked = plan.blockers.length > 0 || !!checkError;
    const waiting = plan.windows.filter(w => w.blockers?.length).length;
    sub = blocked
      ? waiting ? `${waiting} 个窗口待处理，其余窗口保持原样` : '暂时无法切换，请先处理下方提示'
      : n ? `${n} 个窗口先退出，换号后各自恢复` : '没有开着的窗口，直接换号';
    body = (
      <>
        <div className="account-pair"><div><small>当前账号</small><strong>{from?.name || '本机账号登录'}</strong><code>{from?.ip || '出口待核对'}</code></div><QuotaIcon name="arrow" size={22}/><div><small>目标账号</small><strong>{target.name}</strong><code>{target.ip || '出口待核对'}</code></div></div>
        {(plan.globalBlockers ?? plan.blockers).length > 0 && (
          <ul className="blockers" role="alert">
            {(plan.globalBlockers ?? plan.blockers).map((b, i) => <li key={i}><IconAlert /><span>{b}</span></li>)}
          </ul>
        )}
        <div className="switch-monitor"><span role="status">{checking ? '正在检查终端状态…' : '每 2 秒自动检查；处理完成后即可切换'}</span><button type="button" className="btn-text" disabled={checking} onClick={()=>refreshNow.current?.()}>立即检查</button></div>
        {checkError && <p className="form-error" role="alert">状态检查失败：{checkError}</p>}
        {n > 0 && <ul className="term-list switch-windows">{plan.windows.map((w) => <WindowRow key={w.windowId} w={w} pending={locating} onLocate={locate} />)}</ul>}
        {n > 0 && (
          <div className="fieldgroup"><strong>恢复后的权限</strong>
            <div className="form-types permission" role="radiogroup" aria-label="恢复后的权限">{PERMS.map(p =>
              <label key={p.value}><input name="restore-permission" type="radio" value={p.value} checked={perm === p.value} disabled={isRunning} onChange={() => setPerm(p.value)}/>{p.label}</label>
            )}</div>
            <p className="helper">{perm === 'bypassPermissions' ? '全部窗口跳过权限确认。' : '每个窗口保持自己的模型、思考强度和会话记录。'}</p>
          </div>
        )}
      </>
    );
    primary = (
      <button className={'ui-btn '+(perm === 'bypassPermissions' && n ? 'danger' : 'primary')} type="submit" disabled={blocked || !!locating} autoFocus>
        {blocked ? '暂不能切换' : '确认切换'}
      </button>
    );
  } else {
    const { result } = phase;
    const left = result.failed.length;
    sub = result.switched
      ? left ? `已换号，${left} 个窗口没恢复` : `已换号${result.restored ? `，${result.restored} 个窗口已恢复` : ''}`
      : '没有换号';
    body = (
      <>
        {result.errors.length > 0 && (
          <ul className="blockers" role="alert">
            {result.errors.map((e, i) => <li key={i}><IconAlert /><span>{e}</span></li>)}
          </ul>
        )}
        {left > 0 ? (
          <>
            <ul className="win-list">
              {result.failed.map((f) => {
                const w = phase.plan.windows.find(w => w.windowId === f.windowId);
                return <FailureRow key={f.windowId} f={f} busy={retrying !== null || !!locating} spinning={retrying === f.windowId} onRetry={() => retry(result.operationId, f.windowId)} onLocate={w?.canFocus ? () => locate(w) : undefined} />;
              })}
            </ul>
            <p className="flow-foot-note">也可以在那个目录运行 <code>claude --resume</code> 选回它</p>
          </>
        ) : result.switched && !result.errors.length && result.restored > 0 && result.restored === phase.plan.windows.length ? (
          <ul className="term-list result-list">{phase.plan.windows.map(w => <li key={w.windowId}>
            <QuotaIcon name="check" size={16}/><div className="term-id"><strong>{w.name || baseName(w.cwd)}</strong>
              <small>原会话 {w.sessionId.slice(0,8)}{w.model ? ' · '+w.model : ''}{w.effort ? ' · '+w.effort : ''}</small></div><span className="ui-tag good">已恢复</span>
          </li>)}</ul>
        ) : result.switched && !result.errors.length ? (
          <div className="flow-empty ok">{result.restored ? '每个窗口都回到了原来的对话' : '新开的终端会用新号'}</div>
        ) : null}
      </>
    );
    primary = <button className="ui-btn primary" type="submit" disabled={retrying !== null} autoFocus>完成</button>;
  }

  return (
    <Shell
      title={title}
      sub={sub}
      busy={busy}
      onClose={onClose}
      onSubmit={(e) => {
        e.preventDefault();
        if (phase.kind === 'loading' && error) preview();
        else if (phase.kind === 'preview') execute(phase.plan);
        else if (phase.kind === 'done') onClose();
      }}
      foot={isRunning ? null :
        <>
          {toast && <span className="term-toast show" role="status"><QuotaIcon name="check" size={14}/><span>{toast}</span></span>}
          <span className="form-error" role="alert" title={error}>{phase.kind === 'loading' ? '' : error}</span>
          {phase.kind === 'preview' && error && (
            <button className="btn-text" type="button" onClick={preview}>重新检查</button>
          )}
          {phase.kind !== 'done' && <button className="ui-btn" type="button" onClick={onClose} disabled={busy}>取消</button>}
          {primary}
        </>
      }
    >
      {body}
    </Shell>
  );
}
