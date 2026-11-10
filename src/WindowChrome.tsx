import {createContext, useContext, useEffect, useLayoutEffect, useRef, useState, type ReactNode} from 'react';
import {createPortal} from 'react-dom';
import {getCurrentWindow} from '@tauri-apps/api/window';
import {isTauri} from './api';

type WindowState = {maximized: boolean; fullscreen: boolean; focused: boolean};
const WindowStateContext = createContext<WindowState>({maximized: false, fullscreen: false, focused: true});
export const useWindowState = () => useContext(WindowStateContext);

const dialogs: HTMLDialogElement[] = [];
let refreshChrome: (() => void) | undefined;

// A native modal makes everything outside it inert. Move the same chrome host
// into the active dialog so window controls and dragging remain available.
export function attachWindowChrome(dialog: HTMLDialogElement) {
  dialogs.push(dialog);
  refreshChrome?.();
  return () => {
    const index = dialogs.indexOf(dialog);
    if (index !== -1) dialogs.splice(index, 1);
    refreshChrome?.();
  };
}

export function WindowChrome({children}: {children: ReactNode}) {
  const [state, setState] = useState<WindowState>(() => ({maximized: false, fullscreen: false, focused: document.hasFocus()}));
  useEffect(() => {
    const focus = () => setState(s => ({...s, focused: true}));
    const blur = () => setState(s => ({...s, focused: false}));
    window.addEventListener('focus', focus);
    window.addEventListener('blur', blur);
    let disposed = false;
    let revision = 0;
    let off: (() => void) | undefined;
    if (isTauri) {
      const win = getCurrentWindow();
      const sync = async () => {
        const current = ++revision;
        try {
          const [maximized, fullscreen] = await Promise.all([win.isMaximized(), win.isFullscreen()]);
          if (!disposed && current === revision) setState(s => ({...s, maximized, fullscreen}));
        } catch { /* Keep the last known frame state if the native query fails. */ }
      };
      void sync();
      void win.onResized(() => { void sync(); }).then(unlisten => {
        if (disposed) unlisten(); else off = unlisten;
      }).catch(() => {});
    }
    return () => {
      disposed = true;
      off?.();
      window.removeEventListener('focus', focus);
      window.removeEventListener('blur', blur);
    };
  }, []);
  const slot = useRef<HTMLDivElement>(null);
  const [host] = useState(() => {
    const element = document.createElement('div');
    element.className = 'window-chrome';
    return element;
  });
  useLayoutEffect(() => {
    const placeholder = slot.current!;
    placeholder.appendChild(host);
    const update = () => {
      const dialog = dialogs[dialogs.length - 1];
      if (dialog) {
        const bounds = placeholder.getBoundingClientRect();
        if (host.parentElement !== dialog) {
          placeholder.style.height = host.getBoundingClientRect().height + 'px';
          dialog.appendChild(host);
        }
        host.classList.add('is-modal');
        host.style.left = bounds.left + 'px';
        host.style.top = bounds.top + 'px';
        host.style.width = bounds.width + 'px';
        const height = host.getBoundingClientRect().height;
        placeholder.style.height = height + 'px';
        dialog.style.setProperty('--window-chrome-bottom', Math.max(0, bounds.top + height) + 'px');
      } else {
        host.classList.remove('is-modal');
        host.style.removeProperty('left');
        host.style.removeProperty('top');
        host.style.removeProperty('width');
        if (host.parentElement !== placeholder) placeholder.appendChild(host);
        placeholder.style.removeProperty('height');
      }
      const actions = host.querySelector<HTMLElement>('[data-chrome-actions]');
      if (actions) actions.inert = !!dialog;
    };
    refreshChrome = update;
    update();
    const observer = new ResizeObserver(update);
    observer.observe(placeholder);
    observer.observe(host);
    window.addEventListener('resize', update);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', update);
      if (refreshChrome === update) refreshChrome = undefined;
      host.remove();
    };
  }, [host]);
  return <WindowStateContext.Provider value={state}>
    <div className="window-chrome-slot" ref={slot}/>
    {createPortal(<>
      {/* Keep the noninteractive outer edge in the same top layer as the titlebar. */}
      <div aria-hidden="true" hidden={state.maximized || state.fullscreen}
        className={'window-depth-frame' + (isTauri ? ' is-rounded' : '') + (state.focused ? '' : ' is-unfocused')}/>
      {children}
    </>, host)}
  </WindowStateContext.Provider>;
}
