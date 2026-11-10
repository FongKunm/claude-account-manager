import {useEffect, useRef, useState} from 'react';
import {api, errorText, isDemo, type ManagedWindow} from './api';
import {Shell} from './Dialogs';
import {QuotaIcon} from './QuotaIcon';

const statusText: Record<string,string> = {idle:'空闲', busy:'正在工作', waiting:'等待确认', starting:'正在启动', quitting:'正在退出', exited:'已退出'};
export function ManagedTerminals({windows, current, loadError, onClose}: {windows:ManagedWindow[]; current:string; loadError:string; onClose():void}) {
  const [pending,setPending] = useState('');
  const [error,setError] = useState('');
  const [toast,setToast] = useState('');
  const mounted = useRef(false);
  useEffect(() => { mounted.current = true; return () => {mounted.current = false;}; }, []);
  useEffect(() => {
    if (!toast) return;
    let timer: ReturnType<typeof setTimeout>;
    const pause = () => clearTimeout(timer);
    const show = () => {
      pause();
      if (document.hasFocus()) timer = setTimeout(() => setToast(''), 3000);
    };
    // Locating activates the terminal. Keep the notice until the user returns.
    show();
    window.addEventListener('focus', show);
    window.addEventListener('blur', pause);
    return () => {pause();window.removeEventListener('focus', show);window.removeEventListener('blur', pause);};
  }, [toast]);
  async function locate(w:ManagedWindow) {
    setPending(w.windowId);
    setError('');
    setToast('');
    try { await api.focusManagedWindow(w.windowId); if (mounted.current) setToast('已定位到「'+(w.name || '终端')+'」'); }
    catch (err) { if (mounted.current) setError(errorText(err)); }
    finally { if (mounted.current) setPending(''); }
  }
  return <Shell title="受管终端" sub={'当前账号「'+current+'」 · '+windows.length+' 个窗口'} busy={!!pending} onClose={onClose} onSubmit={e=>{e.preventDefault();if (!pending) onClose();}}
    foot={<><span className={'term-toast'+(toast?' show':'')} role="status" aria-live="polite" aria-atomic="true"><QuotaIcon name="check" size={14}/><span>{toast}</span></span><button className="ui-btn" type="submit" disabled={!!pending}>关闭</button></>}>
    {loadError ? <p className="form-error" role="alert">{loadError}</p> : windows.length === 0 ? <p className="helper">当前没有已登记的受管终端。</p> : <ul className="term-list">{windows.map(w=>
      <li key={w.windowId}><span className="term-symbol"><QuotaIcon name="terminal" size={18}/></span>
        <div className="term-id"><strong>{w.name || w.cwd}</strong><small>{w.host || '终端'} · {w.accountName} · <span className="sid">{w.sessionId.slice(0,8)}</span></small><small className="cwd">{w.cwd}</small></div>
        <div className="term-state"><span className={'ui-tag'+(w.hasBackgroundTasks?' amber':'')}>{w.hasBackgroundTasks?'有后台任务':statusText[w.status] || '状态待确认'}</span></div>
        <button type="button" className="ui-btn minor" disabled={!w.canFocus || !!pending} title={w.canFocus?'显示这个终端或标签页':isDemo?'示例会话没有系统窗口':'当前终端无法定位'} onClick={()=>void locate(w)}>{pending===w.windowId?'正在定位…':'定位窗口'} <QuotaIcon name="external" size={13}/></button>
      </li>)}</ul>}
    {error && <p role="alert" className="form-error">{error}</p>}
    {isDemo && <p className="helper">示例会话仅供预览。</p>}
  </Shell>;
}
