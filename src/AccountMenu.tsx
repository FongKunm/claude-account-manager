import {useLayoutEffect, useRef} from 'react';
import {QuotaIcon} from './QuotaIcon';

export interface MenuChoice { label:string; icon:string; aside?:string; run():void; disabled?:boolean; danger?:boolean }
export function AccountMenu({anchor, choices, onClose}: {anchor:HTMLElement; choices:MenuChoice[]; onClose():void}) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(()=>{
    const menu=ref.current;
    if (!menu) return;
    anchor.setAttribute("aria-expanded","true");
    const a=anchor.getBoundingClientRect();
    const minTop=(document.querySelector('.window-chrome-slot')?.getBoundingClientRect().bottom ?? 0)+6;
    menu.style.maxHeight=Math.max(0,innerHeight-minTop-8)+'px';
    menu.style.left=Math.max(8,Math.min(a.right-menu.offsetWidth,window.innerWidth-menu.offsetWidth-8))+'px';
    const top=a.bottom+menu.offsetHeight+8<=innerHeight ? a.bottom+5 : a.top-menu.offsetHeight-4;
    menu.style.top=Math.max(minTop,Math.min(top,innerHeight-menu.offsetHeight-8))+'px';
    menu.querySelector<HTMLButtonElement>('button:enabled')?.focus({preventScroll:true});
    const outside=(e:PointerEvent)=>{if (!menu.contains(e.target as Node) && !anchor.contains(e.target as Node)) onClose();};
    const dismiss=()=>onClose();
    const onScroll=()=>{
      const current=anchor.getBoundingClientRect();
      if (!anchor.isConnected || Math.abs(current.top-a.top)>2 || Math.abs(current.right-a.right)>2) onClose();
    };
    document.addEventListener('pointerdown',outside);
    window.addEventListener('resize',dismiss);
    window.addEventListener('scroll',onScroll,true);
    return ()=>{anchor.setAttribute("aria-expanded","false");document.removeEventListener('pointerdown',outside);window.removeEventListener('resize',dismiss);window.removeEventListener('scroll',onScroll,true);};
  },[anchor,onClose]);
  return <div id="account-menu" role="menu" aria-label="账号操作" ref={ref} onKeyDown={e=>{
    if (e.key==='Escape' || e.key==='Tab') {if(e.key==='Escape')e.preventDefault();onClose();anchor.focus({preventScroll:true});return;}
    if (e.key!=='ArrowDown' && e.key!=='ArrowUp') return;
    e.preventDefault(); const buttons=Array.from(ref.current!.querySelectorAll<HTMLButtonElement>('button:enabled'));
    buttons[(buttons.indexOf(document.activeElement as HTMLButtonElement)+(e.key==='ArrowDown'?1:-1)+buttons.length)%buttons.length]?.focus();
  }}>{choices.map(c=><button type="button" key={c.label} role="menuitem" disabled={c.disabled} className={c.danger?'destructive':undefined} onClick={()=>{onClose();anchor.focus({preventScroll:true});c.run();}}><QuotaIcon name={c.icon} size={15}/> <span>{c.label}</span>{c.aside && <small>{c.aside}</small>}</button>)}</div>;
}
