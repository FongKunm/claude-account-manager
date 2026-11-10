import { getCurrentWindow } from '@tauri-apps/api/window';
import { isTauri } from './api';
import { IconClose, IconMax, IconMin, IconRestore } from './icons';
import { useWindowState } from './WindowChrome';

/** 无边框窗口的最小化 / 最大化 / 关闭；浏览器演示时不显示。 */
export function WindowControls() {
  const {maximized} = useWindowState();

  if (!isTauri) return null;
  const win = getCurrentWindow();
  return (
    <div className="win">
      <button type="button" aria-label="最小化" onClick={() => win.minimize()}><IconMin /></button>
      <button type="button" aria-label={maximized ? '还原' : '最大化'} onClick={() => win.toggleMaximize()}>
        {maximized ? <IconRestore /> : <IconMax />}
      </button>
      <button type="button" className="close" aria-label="关闭" onClick={() => win.close()}><IconClose /></button>
    </div>
  );
}
