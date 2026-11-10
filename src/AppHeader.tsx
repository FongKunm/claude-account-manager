import {QuotaIcon} from './QuotaIcon';
import {WindowChrome} from './WindowChrome';
import {WindowControls} from './WindowControls';

export function AppHeader({ready, checking, refreshDisabled, terminalCount, onRefresh, onTerminals, onAdd}: {
  ready: boolean; checking: boolean; refreshDisabled: boolean; terminalCount: number | null;
  onRefresh(): void; onTerminals(): void; onAdd(): void;
}) {
  return <WindowChrome>
    <header className="top-bar" data-tauri-drag-region>
      <div className="brand-row" data-tauri-drag-region>
        <span className="brand-mark"><QuotaIcon name="spark" size={16}/></span>
        <h1 className="brand-title">Claude 换号台</h1>
        <span className="proto-tag">额度总览</span>
      </div>
      <div className="top-actions top-primary-actions" data-chrome-actions>
        <button type="button" className="btn" title="通过 Claude Code 查询，会用少量额度"
          data-action="refresh" aria-busy={checking || undefined} disabled={refreshDisabled} onClick={onRefresh}>
          <QuotaIcon name="refresh" size={14}/><span>{checking ? '查询中…' : '查当前额度'}</span>
        </button>
        <button type="button" className="btn" data-action="windows" disabled={!ready}
          title={terminalCount === null ? '暂未读取受管终端状态' : undefined} onClick={onTerminals}>
          <QuotaIcon name="terminal" size={14}/><span>{terminalCount !== null ? `受管终端 (${terminalCount})` : '受管终端'}</span>
        </button>
        <button type="button" className="btn btn-clay" data-action="add" disabled={!ready} onClick={onAdd}>
          <QuotaIcon name="plus" size={14}/><span>添加账号</span>
        </button>
      </div>
      <WindowControls/>
    </header>
  </WindowChrome>;
}
