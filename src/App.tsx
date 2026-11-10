import {useCallback, useEffect, useRef, useState} from 'react';
import {listen} from '@tauri-apps/api/event';
import {api, errorText, isDemo, isTauri, type Account, type AppState, type ManagedWindow} from './api';
import {TokenDialog, ExportDialog, Shell, SwitchDialog} from './Dialogs';
import {AccountForm} from './AccountForm';
import {AccountMenu, type MenuChoice} from './AccountMenu';
import {ManagedTerminals} from './ManagedTerminals';
import QuotaBoard from './QuotaBoard';
import {AppHeader} from './AppHeader';
import {SwitchFlow} from './SwitchFlow';
import {buildBoard, DEFAULT_LOGIN, type BoardAccount, type Preferences} from './quotaView';
import {sameIp, skipSwitchTip} from './util';

type Panel = {kind:'add'|'terminals'} | {kind:'edit'|'delete'|'export'; card:BoardAccount} | {kind:'token'; account:Account} | null;
type ActiveMenu = {id:string; anchor:HTMLElement};
function initialPrefs():Preferences {
  let compact = false;
  try { compact = localStorage.getItem('quota-view') === 'compact'; } catch { /* storage may be disabled */ }
  return {view:compact?'compact':'full', sort:'best', filter:'all'};
}
export function App() {
  const [data,setData] = useState<AppState|null>(null);
  const [windows,setWindows] = useState<ManagedWindow[]|null>(null);
  const [windowError,setWindowError] = useState('');
  const [loadError,setLoadError] = useState('');
  const [prefs,setPrefs] = useState(initialPrefs);
  const [query,setQuery] = useState('');
  const [panel,setPanel] = useState<Panel>(null);
  const [menu,setMenu] = useState<ActiveMenu|null>(null);
  const closeMenu = useCallback(()=>setMenu(null),[]);
  const [target,setTarget] = useState<BoardAccount|null>(null);
  const [legacy,setLegacy] = useState(false);
  const [working,setWorking] = useState(false);
  const [panelError,setPanelError] = useState('');
  const [toast,setToast] = useState<{text:string;bad:boolean}|null>(null);
  const lock = useRef(false);
  const loading = useRef(0);
  const listing = useRef(0);
  const tell = (text:string,bad=false)=>setToast({text,bad});
  useEffect(()=>{
    if (!toast) return;
    const t=setTimeout(()=>setToast(null),toast.bad?5500:3000);
    return ()=>clearTimeout(t);
  },[toast]);
  const readState = useCallback(async()=>{
    const seq=++loading.current;
    try { const r=await api.getState(); if (seq===loading.current) {setData(r);setLoadError('');} }
    catch (err) { if(seq===loading.current)setLoadError(errorText(err)); }
  },[]);
  const readWindows = useCallback(async()=>{
    const seq=++listing.current;
    try {const rows=await api.listManagedWindows(); if(seq===listing.current){setWindows(rows);setWindowError('');}}
    catch(err){if(seq===listing.current){setWindows(null);setWindowError(errorText(err));}}
  },[]);
  const reload = useCallback(()=>{void readState();void readWindows();},[readState,readWindows]);
  useEffect(()=>{
    reload();
    const t=setInterval(reload,30_000);
    window.addEventListener('focus',reload);
    const off=isTauri ? listen('usage-updated',()=>void readState()) : null;
    return ()=>{++loading.current;++listing.current;clearInterval(t);window.removeEventListener('focus',reload);off?.then(f=>f(),()=>{});};
  },[reload,readState]);
  const board=data && buildBoard(data,prefs,query?{[query]:{usage:true}}:{},windows?.length??null);
  const card=(id:string)=>board?.current?.id===id?board.current:(board?.items.find(c=>c.id===id) ?? (board?.recommended?.id===id ? board.recommended : null));
  async function check(id:string) {
    if (lock.current) return;
    lock.current=true;setQuery(id);
    try {const result=id===DEFAULT_LOGIN?await api.checkNormalUsage():await api.checkUsage(id);
      if(result.ok || result.state==='limited')tell('额度已更新');else tell(result.error || '本次没有取得额度',true);
    } catch(err){tell(errorText(err),true);}
    finally{lock.current=false;setQuery('');void readState();}
  }
  async function checkExit(c:BoardAccount) {
    try {
      if(c.id===DEFAULT_LOGIN){
        const r=await api.detectExit(c.proxy);
        if(c.ip && !sameIp(c.ip,r.ip))throw '实测出口与绑定的 IP 不一致';
        tell((c.ip?'出口一致 · ':'实测出口 · ')+r.ip);
      } else {
        const r=await api.checkNetwork(c.id);
        if(!r.ok)throw r.error || '出口核对失败';
        tell('出口一致 · '+r.ip);
      }
    }catch(err){tell(errorText(err),true);}finally{void readState();}
  }
  const open=(next:Panel)=>{setPanelError('');setPanel(next);};
  const done=(msg:string)=>{setPanel(null);tell(msg);reload();};
  const openTerminals=()=>{open({kind:'terminals'});void readWindows();};
  const menuCard=menu && card(menu.id);
  const choices:MenuChoice[]=[];
  if(menuCard && data){
    const c=menuCard;
    const token=data.accounts.find(a=>a.id===c.id);
    choices.push({label:'查询额度',icon:'refresh',aside:'会用少量额度',disabled:!!query,run:()=>void check(c.id)},
      {label:'检查出口',icon:'globe',run:()=>void checkExit(c)});
    if(c.id!==DEFAULT_LOGIN)choices.push({label:'编辑账号',icon:'edit',run:()=>open({kind:'edit',card:c})});
    if(token)choices.push({label:'更换令牌',icon:'key',run:()=>open({kind:'token',account:token})});
    if(token || c.id===DEFAULT_LOGIN)choices.push({label:'导出 CPA JSON',icon:'external',run:()=>open({kind:'export',card:c})});
    if(c.id!==DEFAULT_LOGIN)choices.push({label:'删除账号',icon:'trash',danger:true,disabled:c.id===board?.current?.id,run:()=>open({kind:'delete',card:c})});
  }
  async function remove(c:BoardAccount) {
    if(working)return;setWorking(true);
    try{await api.deleteAccount(c.id);done('已删除「'+c.name+'」');}catch(err){setPanelError(errorText(err));void readState();}finally{setWorking(false);}
  }
  async function oldSwitch(c:BoardAccount){
    if(working)return;setWorking(true);
    try{if(c.id===DEFAULT_LOGIN)await api.useNormal();else await api.switchTo(c.id);setTarget(null);setLegacy(false);reload();tell('已切换，新终端使用所选账号');}
    catch(err){tell(errorText(err),true);}finally{setWorking(false);}
  }
  const invokeLocal=(f:()=>Promise<void>)=>void f().catch(err=>tell(errorText(err),true));
  return <>
    <div className="app-shell">
    <AppHeader ready={!!data} checking={!!board?.current?.checking}
      refreshDisabled={!board?.current || !!query || !!board.current.checking}
      terminalCount={windows?.length??null} onRefresh={()=>{if(board?.current)void check(board.current.id);}}
      onTerminals={openTerminals} onAdd={()=>open({kind:'add'})}/>
    <main className="app-content" aria-label="额度总览" tabIndex={0}>
    {board && data ? <QuotaBoard state={board} actions={{
      query:id=>void check(id),menu:(id,anchor)=>setMenu(m=>m?.id===id?null:{id,anchor}),
      switchTo:id=>{const c=card(id);if(c){setLegacy(false);setTarget(c);}},
      add:()=>open({kind:'add'}),terminals:openTerminals,anyQuery:!!query,
      setView:view=>{setPrefs(s=>({...s,view}));try{localStorage.setItem('quota-view',view);}catch{/* optional */}},
      setSort:sort=>setPrefs(s=>({...s,sort})),setFilter:filter=>setPrefs(s=>({...s,filter})),
      alerts: <>{loadError && <p className="conflict-alert" role="alert">{loadError}</p>}{data.conflicts.length>0 && <p className="conflict-alert" role="alert">检测到影响账号或代理的设置 · {data.conflicts.map(c=>c.source+' / '+c.key).join('、')}</p>}</>,
      footer:<footer className="desk-footer"><span>{isDemo?'本地演示 · 示例数据':'Claude 换号台 · v'+data.version}</span><div><button onClick={()=>invokeLocal(api.openDataFolder)}>数据目录</button><button onClick={()=>invokeLocal(api.openHelp)}>使用说明</button></div></footer>
    }}/> : <div className="board"><p role={loadError?'alert':'status'}>{loadError || '正在读取本机账号…'}</p>{loadError && <button className="btn" onClick={reload}>重试</button>}</div>}
    </main>
    </div>
    {menu && menuCard && <AccountMenu anchor={menu.anchor} choices={choices} onClose={closeMenu}/>}
    {(panel?.kind==='add'||panel?.kind==='edit') && data && <AccountForm state={data} account={panel.kind==='edit'?panel.card:undefined} onClose={()=>open(null)} onDone={done}/>}
    {panel?.kind==='token' && data && <TokenDialog account={panel.account} onClose={()=>open(null)} onDone={done}/>}
    {panel?.kind==='export' && <ExportDialog target={{id:panel.card.id===DEFAULT_LOGIN?null:panel.card.id,name:panel.card.name}} onClose={()=>open(null)}/>}
    {panel?.kind==='terminals' && <ManagedTerminals windows={windows??[]} current={board?.current?.name??'待确认'} loadError={windowError} onClose={()=>open(null)}/>}
    {panel?.kind==='delete' && <Shell title={'删除「'+panel.card.name+'」'} sub="这会移除换号台保存的账号资料。" busy={working} onClose={()=>open(null)} onSubmit={e=>{e.preventDefault();void remove(panel.card);}} foot={<><button className="ui-btn" type="button" disabled={working} onClick={()=>open(null)}>取消</button><button className="ui-btn danger" type="submit" disabled={working}>确认删除</button></>}><p>固定出口和本地额度记录将一起移除。</p>{panelError && <p role="alert" className="form-error">{panelError}</p>}</Shell>}
    {target && !legacy && <SwitchFlow from={board?.current} target={{id:target.id===DEFAULT_LOGIN?null:target.id,name:target.name,ip:target.ip}} onClose={()=>{setTarget(null);reload();}} onSwitched={reload} onUnsupported={()=>{
      if(target.method==='账号登录' && target.id!==DEFAULT_LOGIN){setTarget(null);tell('这台电脑暂不支持切到登记的账号',true);return;}
      setLegacy(true);if(skipSwitchTip())void oldSwitch(target);
    }}/>}
    {target && legacy && <SwitchDialog target={{name:target.name,normal:target.id===DEFAULT_LOGIN}} currentExhausted={board?.current?.status==='额度用完'} busy={working} onCancel={()=>setTarget(null)} onConfirm={()=>void oldSwitch(target)}/>}
    <div id="notice" role="status" aria-live="polite" className={toast?'show'+(toast.bad?' bad':''):''}>{toast?.text}</div>
  </>;
}
