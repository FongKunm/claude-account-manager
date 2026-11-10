import { useId, useLayoutEffect, useRef, useState } from 'react';
import { api, errorText, type Account } from './api';
import { IconClose, IconCopy, IconEye, IconEyeOff, IconSwap } from './icons';
import { setSkipSwitchTip } from './util';
import { attachWindowChrome } from './WindowChrome';

export function Shell({ title, sub, busy, onClose, children, foot, onSubmit, width }: {
  title: string; sub?: string; busy: boolean; onClose(): void; children: React.ReactNode; foot: React.ReactNode; onSubmit(e: React.FormEvent): void; width?: number;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const heading = useId();
  useLayoutEffect(() => {
    const el = dialog.current;
    if (!el) return;
    const previousFocus = document.activeElement;
    el.showModal();
    const releaseChrome = attachWindowChrome(el);
    return () => {
      el.close();
      releaseChrome();
      const restoreFocus = () => {
        if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus({preventScroll:true});
      };
      restoreFocus();
      // React may restore a chrome button moved during this commit. Return to
      // the dialog's trigger after that restoration, unless it reopened.
      queueMicrotask(() => {if (!el.open) restoreFocus();});
    };
  }, []);
  return (
    <dialog ref={dialog} className="ui-dialog" aria-labelledby={heading} style={width ? {width: 'min(' + width + 'px, calc(100vw - 28px))'} : undefined}
      onCancel={e => {e.preventDefault(); if (!busy) onClose();}}
      onClick={e => { if (e.target === e.currentTarget && !busy) {
        const r = e.currentTarget.getBoundingClientRect();
        if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) onClose();
      }}}>
      <form className="sheet" onSubmit={onSubmit} noValidate autoComplete="off">
        <header className="ui-head"><div><h2 id={heading}>{title}</h2>{sub && <p>{sub}</p>}</div>
          {!busy && <button type="button" className="ui-close" onClick={onClose} aria-label="关闭"><IconClose /></button>}
        </header>
        <div className="ui-content">{children}</div>
        {foot && <footer className="ui-foot">{foot}</footer>}
      </form>
    </dialog>
  );
}

// ———— 令牌输入 + 获取步骤 ————

function TokenGuide() {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText('claude setup-token');
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch { /* 忽略 */ }
  }
  return (
    <ol className="guide">
      <li>
        <span>在终端运行</span>
        <button type="button" className="cmd" onClick={copy} title="复制">
          <code>claude setup-token</code>{copied ? <span className="ok">已复制</span> : <IconCopy />}
        </button>
      </li>
      <li><span>浏览器里登录<b>要添加的那个号</b>并授权<em>登着别的号就用无痕窗口</em></span></li>
      <li><span>把终端打出的令牌粘贴到下面</span></li>
    </ol>
  );
}

function TokenField({ value, onChange, autoFocus }: { value: string; onChange(v: string): void; autoFocus?: boolean }) {
  const [show, setShow] = useState(false);
  return (
    <label className="field">
      <span>新的 Setup Token</span>
      <div className="input-icon">
        <input
          className="mono"
          type={show ? 'text' : 'password'}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="sk-ant-oat01-…"
          spellCheck={false}
          autoFocus={autoFocus}
        />
        <button type="button" className="icon-btn sm" onClick={() => setShow(!show)} aria-label={show ? '隐藏' : '显示'}>
          {show ? <IconEyeOff /> : <IconEye />}
        </button>
      </div>
    </label>
  );
}

// ———— 添加 / 编辑 / 更换令牌 ————

export function TokenDialog({ account, onClose, onDone }: {
  account: Account; onClose(): void; onDone(msg: string): void;
}) {
  const [token, setToken] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const saving = useRef(false);
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (saving.current || !token.trim()) return;
    saving.current = true;
    setBusy(true);
    setError('');
    try {
      await api.replaceToken(account.id, token.trim());
      onDone('令牌已更换');
    } catch (err) {
      setError(errorText(err));
    } finally {
      saving.current = false;
      setBusy(false);
    }
  }
  return <Shell title="更换令牌" sub={account.name} busy={busy} onClose={onClose} onSubmit={submit} foot={<>
    <button className="ui-btn" type="button" disabled={busy} onClick={onClose}>取消</button>
    <button className="ui-btn primary" type="submit" disabled={busy || !token.trim()}>{busy ? '正在更换…' : '更换令牌'}</button>
  </>}>
    <TokenField value={token} onChange={setToken} autoFocus/>
    <p className="helper">更换后需重新读取额度。</p>
    <details><summary>如何获取 Setup Token</summary><TokenGuide/></details>
    {error && <p className="ui-error" role="alert">{error}</p>}
  </Shell>;
}

// ———— 切换前说明 ————

export function SwitchDialog({ target, currentExhausted, busy, onCancel, onConfirm }: {
  target: { name: string; normal: boolean };
  currentExhausted: boolean;
  busy: boolean;
  onCancel(): void;
  onConfirm(): void;
}) {
  const [skip, setSkip] = useState(false);
  return (
    <Shell
      title={target.normal ? '切回账号登录' : `切换到「${target.name}」`}
      busy={busy}
      onClose={onCancel}
      width={460}
      onSubmit={(e) => {
        e.preventDefault();
        setSkipSwitchTip(skip);
        onConfirm();
      }}
      foot={
        <>
          <label className="check">
            <input type="checkbox" checked={skip} onChange={(e) => setSkip(e.target.checked)} />
            <span>以后不再提示</span>
          </label>
          <button className="btn-text" type="button" onClick={onCancel} disabled={busy}>取消</button>
          <button className="btn btn-white" type="submit" disabled={busy} autoFocus>
            {busy ? <span className="spinner" /> : <IconSwap />}切换
          </button>
        </>
      }
    >
      <ul className="tips">
        <li><b>运行中的对话</b>继续用原来的号，不会中断</li>
        <li><b>新开的终端</b>、重开的 VS Code 用新号</li>
        <li><b>已开的 PowerShell</b> 里重新运行 claude 即可</li>
        <li>当前对话也换号：先 <code>/stop</code>，再 <code>claude --resume</code> 选回它<em className="tip-sub"><code>/exit</code> 可能只断开、旧号还在后台</em></li>
        {currentExhausted && <li className="warn">当前号额度已用完，旧对话会报限流</li>}
      </ul>
    </Shell>
  );
}

// ———— 导出 CPA JSON ————

export function ExportDialog({ target, onClose }: {
  /** id 为 null 是账号登录 */
  target: { id: string | null; name: string };
  onClose(): void;
}) {
  const normal = target.id === null;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [path, setPath] = useState('');
  const [copied, setCopied] = useState(false);

  async function save(e: React.FormEvent) {
    e.preventDefault();
    if (path) return onClose();
    setBusy(true);
    setError('');
    try {
      const r = await api.exportCpa(target.id);
      if (r) setPath(r.path);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(path);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch { /* 忽略 */ }
  }

  return (
    <Shell
      title="导出 CPA JSON"
      sub={normal ? `账号登录 · ${target.name}` : target.name}
      busy={busy}
      onClose={onClose}
      width={480}
      onSubmit={save}
      foot={
        <>
          <span className="form-error" role="alert" title={error}>{error}</span>
          {!path && <button className="btn-text" type="button" onClick={onClose} disabled={busy}>取消</button>}
          <button className="btn btn-white" type="submit" disabled={busy} autoFocus>
            {busy && <span className="spinner" />}{path ? '完成' : '选择保存位置'}
          </button>
        </>
      }
    >
      {path ? (
        <label className="field">
          <span className="ok-text">已导出到</span>
          <div className="input-icon input-action">
            <input className="mono" value={path} readOnly onFocus={(e) => e.target.select()} spellCheck={false} />
            <button type="button" className="in-btn" onClick={copy}>{copied ? '已复制' : '复制'}</button>
          </div>
        </label>
      ) : (
        <ul className="tips">
          <li className="warn">文件含<b>完整凭据</b>，别存 OneDrive、别发给别人</li>
          <li>导入 CPA 后默认<b>停用</b>，并带一个连不上的阻断代理</li>
          <li>先换成这个号在 CPA 的代理，核对固定出口，再启用</li>
          {normal && <li className="warn">含续期令牌：CPA 和本机同时续期，本机登录可能失效</li>}
          {normal && <li>换号台只读本机登录，不续期、不改写</li>}
        </ul>
      )}
    </Shell>
  );
}
