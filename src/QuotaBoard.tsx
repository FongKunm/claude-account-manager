import type { ChangeEvent, MouseEvent } from 'react';
import { DEFAULT_LOGIN, type BoardAccount, type BoardBucket, type BoardState } from './quotaView';
import { QuotaIcon } from './QuotaIcon';

export type BoardActions = {
  query: (id: string) => void;
  menu: (id: string, anchor: HTMLElement) => void;
  switchTo: (id: string) => void;
  add: () => void;
  terminals: () => void;
  setView: (v: BoardState['view']) => void;
  setSort: (v: BoardState['sort']) => void;
  setFilter: (v: BoardState['filter']) => void;
  anyQuery: boolean;
  footer?: React.ReactNode;
  alerts?: React.ReactNode;
};

type DialTick = {
  index: number;
  x1: string;
  y1: string;
  x2: string;
  y2: string;
  major: boolean;
};

const DIAL_CX = 92;
const DIAL_CY = 86;
const DIAL_R = 64;
const DIAL_TRACK_PATH = 'M 28 86 A 64 64 0 0 1 156 86';

function polar(cx: number, cy: number, r: number, deg: number): { x: string; y: string } {
  const rad = (deg * Math.PI) / 180;
  return {
    x: (cx + r * Math.cos(rad)).toFixed(2),
    y: (cy + r * Math.sin(rad)).toFixed(2),
  };
}

const DIAL_TICKS: readonly DialTick[] = Array.from({ length: 21 }, (_, i) => {
  const deg = 180 + (i / 20) * 180;
  const major = i % 5 === 0;
  const p1 = polar(DIAL_CX, DIAL_CY, DIAL_R + 8, deg);
  const p2 = polar(DIAL_CX, DIAL_CY, DIAL_R + (major ? 13.5 : 10.5), deg);
  return {
    index: i,
    x1: p1.x,
    y1: p1.y,
    x2: p2.x,
    y2: p2.y,
    major,
  };
});

const FILTER_OPTIONS: ReadonlyArray<{ value: BoardState['filter']; label: string }> = [
  { value: 'all', label: '全部' },
  { value: 'switchable', label: '可切换' },
  { value: 'login', label: '账号登录' },
  { value: 'token', label: 'Setup Token' },
];

const SORT_OPTIONS: ReadonlyArray<{ value: BoardState['sort']; label: string }> = [
  { value: 'best', label: '推荐优先' },
  { value: 'reset', label: '最早重置' },
  { value: 'remaining', label: '剩余最多' },
];

function validNum(v: number | null | undefined): v is number {
  return typeof v === 'number' && !Number.isNaN(v);
}

function clampPct(v: number): number {
  return Math.max(0, Math.min(100, Math.round(v)));
}

function hasReadableQuota(bucket: BoardBucket | null | undefined): bucket is BoardBucket & { remaining: number } {
  return Boolean(bucket && validNum(bucket.remaining) && bucket.tone !== 'unknown');
}

function isUltraAccount(account: BoardAccount): boolean {
  return account.short === null || account.plan === 'Ultra';
}

function hasRealFable(fable: BoardBucket | null | undefined): fable is BoardBucket {
  if (!fable) return false;
  return validNum(fable.remaining) || validNum(fable.used) || (fable.tone !== 'unknown' && Boolean(fable.reset));
}

function resetToneClass(tone: BoardBucket['tone'] | undefined): string {
  if (tone === 'ok') return 't-strong';
  if (tone === 'soon') return 't-soon';
  return '';
}

function getAccountInitial(account: BoardAccount): string {
  const raw = (account.email || account.name || '').trim();
  return raw ? raw[0].toUpperCase() : '?';
}

function QuotaDial({
  label,
  bucket,
  accentClay,
}: {
  label: string;
  bucket: BoardBucket | null;
  accentClay: boolean;
}) {
  const hasRem = hasReadableQuota(bucket);
  const rem = hasRem ? clampPct(bucket.remaining) : null;
  const used =
    bucket && validNum(bucket.used)
      ? clampPct(bucket.used)
      : rem !== null
        ? Math.max(0, 100 - rem)
        : null;
  const resetTxt = bucket?.reset || '—';
  const resetTitle = bucket?.at || resetTxt;
  const toneCls = resetToneClass(bucket?.tone);
  const isLow = hasRem && (rem! <= 20);

  const endPt = hasRem && rem! > 0 ? polar(DIAL_CX, DIAL_CY, DIAL_R, 180 + (rem! / 100) * 180) : null;
  const strokeColor = accentClay || isLow ? '#d97757' : '#eae0d0';

  return (
    <div className="dial-box">
      <div className="dial-hd">
        <span>{label}</span>
        <span className="dial-used-sub">{hasRem && used !== null ? `已用 ${used}%` : '读数未知'}</span>
      </div>
      <div className="dial-svg-wrap">
        <svg viewBox="0 0 184 98" aria-hidden="true">
          {DIAL_TICKS.map((t) => (
            <line
              key={t.index}
              x1={t.x1}
              y1={t.y1}
              x2={t.x2}
              y2={t.y2}
              stroke={t.major ? 'rgba(243,237,226,0.28)' : 'rgba(243,237,226,0.11)'}
              strokeWidth={t.major ? '1.5' : '1'}
              strokeLinecap="round"
            />
          ))}
          <path
            d={DIAL_TRACK_PATH}
            fill="none"
            stroke="rgba(243,237,226,0.09)"
            strokeWidth="8"
            strokeLinecap="round"
            strokeDasharray={hasRem ? undefined : '4 5'}
          />
          {endPt && (
            <>
              <path
                className="dial-arc"
                d={`M 28 86 A 64 64 0 0 1 ${endPt.x} ${endPt.y}`}
                fill="none"
                stroke={strokeColor}
                strokeWidth="8"
                strokeLinecap="round"
              />
              <circle
                cx={endPt.x}
                cy={endPt.y}
                r="3.8"
                fill="#1a1715"
                stroke={strokeColor}
                strokeWidth="2.2"
              />
            </>
          )}
        </svg>
        <div className="dial-center">
          {hasRem && rem !== null ? (
            <>
              <div className="dial-val">
                {rem}
                <span className="u">%</span>
              </div>
              <div className="dial-sub-lbl">剩余可用</div>
            </>
          ) : (
            <>
              <div className="dial-val">—</div>
              <div className="dial-sub-lbl">剩余未知</div>
            </>
          )}
        </div>
      </div>
      <div className="dial-foot">
        <QuotaIcon name="clock" size={13} />
        <span className={toneCls || undefined} title={resetTitle}>
          {resetTxt}
        </span>
      </div>
    </div>
  );
}

function BucketBar({
  label,
  bucket,
  isRec,
}: {
  label: string;
  bucket: BoardBucket | null;
  isRec: boolean;
}) {
  const hasRem = hasReadableQuota(bucket);
  const rem = hasRem ? clampPct(bucket.remaining) : null;
  const isLow = hasRem && (rem! <= 18);
  const resetTxt = bucket?.reset || '—';
  const resetTitle = bucket?.at || resetTxt;
  const toneCls = resetToneClass(bucket?.tone);

  return (
    <div className="b-row">
      <div className="b-top">
        <span className="b-lbl">
          <span>{label}</span>
          {hasRem && rem !== null ? (
            <span className={`b-pct${isLow ? ' is-low' : ''}`}>{rem}%</span>
          ) : (
            <span className="b-pct is-unknown">—</span>
          )}
        </span>
        <span className={`b-reset${toneCls ? ` ${toneCls}` : ''}`} title={resetTitle}>
          {resetTxt}
        </span>
      </div>
      <div className={`b-track${hasRem ? '' : ' is-unknown'}`}>
        {hasRem && rem !== null && (
          <div
            className={`b-fill${isLow ? ' is-low' : isRec ? ' is-rec-bar' : ''}`}
            style={{ width: `${rem}%` }}
          />
        )}
      </div>
    </div>
  );
}

function CompactQuotaCell({
  mobileLabel,
  bucket,
  omitSlot,
  isRec,
}: {
  mobileLabel: string;
  bucket: BoardBucket | null;
  omitSlot?: boolean;
  isRec: boolean;
}) {
  if (omitSlot) {
    return <div className="cp-q is-empty" aria-hidden="true" />;
  }

  const hasRem = hasReadableQuota(bucket);
  const rem = hasRem ? clampPct(bucket.remaining) : null;
  const isLow = hasRem && (rem! <= 18);
  const resetTxt = bucket?.reset || '—';
  const resetTitle = bucket?.at || resetTxt;
  const toneCls = resetToneClass(bucket?.tone);

  return (
    <div className="cp-q">
      <div className="cp-q-line">
        <span>
          <span className="cp-q-mb-lbl">{mobileLabel}</span>
          <span className={`cp-q-pct${hasRem ? (isLow ? ' is-low' : '') : ' is-unknown'}`}>
            {hasRem && rem !== null ? `${rem}%` : '—'}
          </span>
        </span>
        <span className={`cp-q-reset${toneCls ? ` ${toneCls}` : ''}`} title={resetTitle}>
          {resetTxt}
        </span>
      </div>
      <div className={`b-track${hasRem ? '' : ' is-unknown'}`}>
        {hasRem && rem !== null && (
          <div
            className={`b-fill${isLow ? ' is-low' : isRec ? ' is-rec-bar' : ''}`}
            style={{ width: `${rem}%` }}
          />
        )}
      </div>
    </div>
  );
}

function EndpointLabel({ ip, port, hero }: { ip: string; port: string; hero?: boolean }) {
  const ipText = ip || '未配置出口';
  const portCls = hero ? 'egress-port' : 'local-port';
  return (
    <span>
      {ipText}
      {port ? <>{hero ? '' : ' '}<span className={portCls}>本机 {port}</span></> : null}
    </span>
  );
}

function HeroSection({
  current,
  terminalCount,
  actions,
}: {
  current: BoardAccount | null;
  terminalCount: number | null;
  actions: BoardActions;
}) {
  if (!current) {
    return (
      <section className="hero-card" aria-label="当前生效账号">
        <div className="hero-left">
          <div>
            <div className="hero-top-line">
              <div className="hero-badges">
                <span className="pill pill-neutral">暂无当前账号</span>
              </div>
            </div>
            <div className="hero-identity">
              <span className="account-initial" aria-hidden="true">
                —
              </span>
              <div className="hero-text">
                <h2 className="hero-name">尚未识别生效账号</h2>
                <div className="hero-email">请添加账号或从下方候选列表切换</div>
              </div>
            </div>
          </div>
          <div className="term-bar">
            <span className="cam-read">未读取到当前账号读数</span>
            <button
              type="button"
              className="btn btn-sm"
              data-action="windows"
              title={terminalCount === null ? '暂未读取受管终端状态' : undefined}
              onClick={actions.terminals}
            >
              <QuotaIcon name="terminal" size={13} />
              <span>{terminalCount !== null ? `查看 ${terminalCount} 个终端` : '终端未就绪'}</span>
            </button>
          </div>
        </div>
        <div className="hero-dials is-single">
          <QuotaDial label="7 天额度" bucket={null} accentClay={false} />
        </div>
      </section>
    );
  }

  const singleDial = isUltraAccount(current);
  const email = /^[^\s@]+@[^\s@]+$/.test(current.email) ? current.email : '';
  const identity = email || current.name;
  const identityDetail = current.id === DEFAULT_LOGIN && email ? '本机账号登录'
    : email ? (current.name !== email ? current.name : '') : current.email;
  const showFable = hasRealFable(current.fable);
  const handleMenu = (e: MouseEvent<HTMLButtonElement>) => {
    actions.menu(current.id, e.currentTarget);
  };

  return (
    <section className="hero-card" data-current={current.id} aria-label="当前生效账号">
      <div className="hero-left">
        <div>
          <div className="hero-top-line">
            <div className="hero-badges">
              <span className="pill pill-live">
                <span className="dot" />
                正在使用
              </span>
              {current.plan ? <span className="meta-chip is-plan">{current.plan}</span> : null}
              <span className="meta-chip">{current.method}</span>
            </div>
            <button
              type="button"
              className="icon-btn"
              aria-haspopup="menu"
              title="账号管理"
              data-action="menu"
              data-id={current.id}
              aria-label={`管理${current.name}`}
              onClick={handleMenu}
            >
              <QuotaIcon name="more" size={16} />
            </button>
          </div>

          <div className="hero-identity">
            <span className="account-initial" aria-hidden="true">
              {getAccountInitial(current)}
            </span>
            <div className="hero-text">
              <h2 className={'hero-name'+(email?' is-email':'')}>{identity}</h2>
              {identityDetail ? <div className="hero-subtitle">{identityDetail}</div> : null}
            </div>
          </div>

          {current.note ? (
            <div className="hero-note" title={current.note}>
              {current.note}
            </div>
          ) : null}
        </div>

        <div className="egress-strip">
          <span className="egress-addr">
            <QuotaIcon name="globe" size={14} />
            <EndpointLabel ip={current.ip} port={current.port} hero />
          </span>
          {current.verified ? (
            <span className="pill pill-live">
              <QuotaIcon name="check" size={12} />
              固定出口
            </span>
          ) : (
            <span className="pill pill-neutral">{current.status || '出口待核对'}</span>
          )}
        </div>

        <div className="term-bar">
          <span className="cam-read">
            {current.checking ? '正在查询额度…' : current.updated || current.status || '尚无额度记录'}
          </span>
          <button
            type="button"
            className="btn btn-sm"
            data-action="windows"
            title={terminalCount === null ? '暂未读取受管终端状态' : undefined}
            onClick={actions.terminals}
          >
            <QuotaIcon name="terminal" size={13} />
            <span>{terminalCount !== null ? `查看 ${terminalCount} 个终端` : '终端待读取'}</span>
          </button>
        </div>
      </div>

      <div className={`hero-dials${singleDial ? ' is-single' : ''}`}>
        {!singleDial ? <QuotaDial label="5 小时额度" bucket={current.short} accentClay /> : null}
        <QuotaDial label="7 天额度" bucket={current.week} accentClay={singleDial} />
        {showFable ? (
          <div style={{ gridColumn: '1 / -1' }}>
            <BucketBar label="Fable 额度" bucket={current.fable} isRec={false} />
          </div>
        ) : null}
      </div>
    </section>
  );
}

function CandidateCard({
  account,
  isRec,
  recReason,
  actions,
}: {
  account: BoardAccount;
  isRec: boolean;
  recReason: string;
  actions: BoardActions;
}) {
  const ultra = isUltraAccount(account);
  const showFable = hasRealFable(account.fable);
  const queryBusy = actions.anyQuery || account.checking;

  return (
    <article
      className={`cand-card${isRec ? ' is-rec' : ''}${!account.switchable ? ' is-unavailable' : ''}`}
      data-account={account.id}
    >
      <div>
        <div className="card-hd">
          <div className="card-id">
            <div className="card-title-row">
              <h3 className="card-name">{account.name}</h3>
              {isRec ? (
                <span className="pill pill-rec">
                  <QuotaIcon name="spark" size={12} />
                  推荐切换
                </span>
              ) : null}
            </div>
            {account.email ? <div className="card-email">{account.email}</div> : null}
            {account.note ? (
              <div className="card-note" title={account.note}>
                {account.note}
              </div>
            ) : null}
          </div>

          <div className="card-tools">
            <button
              type="button"
              className="icon-btn"
              title="查询额度（会用少量额度）"
              data-action="refresh"
              data-id={account.id}
              aria-label={`查询${account.name}的额度（会用少量额度）`}
              aria-busy={account.checking || undefined}
              disabled={queryBusy}
              onClick={() => actions.query(account.id)}
            >
              <QuotaIcon name="refresh" size={14} />
            </button>
            <button
              type="button"
              className="icon-btn"
              aria-haspopup="menu"
              title="账号管理"
              data-action="menu"
              data-id={account.id}
              aria-label={`管理${account.name}`}
              onClick={(e) => actions.menu(account.id, e.currentTarget)}
            >
              <QuotaIcon name="more" size={15} />
            </button>
          </div>
        </div>

        {isRec && recReason ? (
          <div className="rec-banner" style={{ marginTop: 9 }}>
            <QuotaIcon name="star" size={13} />
            <span title={recReason}>{recReason}</span>
          </div>
        ) : null}

        <div className="card-meta-bar" style={{ marginTop: 10 }}>
          <span className="mono-ep">
            <QuotaIcon name="globe" size={13} />
            <EndpointLabel ip={account.ip} port={account.port} />
          </span>
          <span style={{ display: 'inline-flex', gap: 5, alignItems: 'center' }}>
            {account.plan ? <span className="meta-chip is-plan">{account.plan}</span> : null}
            <span className="meta-chip">{account.method}</span>
          </span>
        </div>

        <div className="buckets-stack" style={{ marginTop: 11 }}>
          {!ultra ? <BucketBar label="5 小时剩余" bucket={account.short} isRec={isRec} /> : null}
          <BucketBar label="7 天剩余" bucket={account.week} isRec={isRec} />
          {showFable ? <BucketBar label="Fable 剩余" bucket={account.fable} isRec={isRec} /> : null}
        </div>
      </div>

      <div className="card-ft">
        <span
          className={`pill ${account.switchable ? 'pill-neutral' : 'pill-bad'}`}
          title={account.updated || undefined}
        >
          {account.checking ? '正在查询…' : account.status || (account.switchable ? '可切换' : '不可切换')}
        </span>
        <button
          type="button"
          className={`btn btn-sm${isRec && account.switchable ? ' btn-rec' : ''}`}
          data-action="switch"
          data-id={account.id}
          disabled={!account.switchable}
          onClick={() => account.switchable && actions.switchTo(account.id)}
        >
          <QuotaIcon name="swap" size={13} />
          <span>{account.switchable ? (isRec ? '推荐切换' : '切换账号') : '不可切换'}</span>
        </button>
      </div>
    </article>
  );
}

function CompactRow({
  account,
  isRec,
  recReason,
  actions,
}: {
  account: BoardAccount;
  isRec: boolean;
  recReason: string;
  actions: BoardActions;
}) {
  const ultra = isUltraAccount(account);
  const fableBucket = hasRealFable(account.fable) ? account.fable : null;
  const fableText =
    fableBucket !== null
      ? hasReadableQuota(fableBucket)
        ? `Fable 剩余 ${clampPct(fableBucket.remaining)}%`
        : `Fable ${fableBucket.reset || '—'}`
      : '';
  const subParts = [account.email, account.note, fableText].filter(Boolean).join(' · ');
  const queryBusy = actions.anyQuery || account.checking;

  return (
    <div className={`cp-row${isRec ? ' is-rec' : ''}`} data-account={account.id}>
      <div className="cp-id">
        <div className="cp-name-line">
          <span className="cp-name">{account.name}</span>
          {isRec ? (
            <span className="pill pill-rec">
              <QuotaIcon name="spark" size={11} />
              推荐切换
            </span>
          ) : null}
          {account.plan ? <span className="meta-chip is-plan">{account.plan}</span> : null}
          <span className="meta-chip">{account.method}</span>
        </div>
        {(isRec && recReason) || subParts ? (
          <div className="cp-sub" title={subParts || undefined}>
            {isRec && recReason ? <span className="rec-inline">{recReason}</span> : null}
            {subParts}
          </div>
        ) : null}
      </div>

      <div className="cp-ep">
        <span className="mono-ep">{account.ip || '未配置出口'}{account.port && <> <span className="local-port">本机 {account.port}</span></>}</span>
        <span className="cp-ep-status" title={account.updated || undefined}>
          {account.checking ? '正在查询…' : account.status || (account.switchable ? '可切换' : '不可切换')}
        </span>
      </div>

      <CompactQuotaCell
        mobileLabel="5小时剩余"
        bucket={account.short}
        omitSlot={ultra}
        isRec={isRec}
      />
      <CompactQuotaCell
        mobileLabel="7天剩余"
        bucket={account.week}
        isRec={isRec}
      />

      <div className="cp-act">
        <button
          type="button"
          className={`btn btn-sm${isRec && account.switchable ? ' btn-rec' : ''}`}
          data-action="switch"
          data-id={account.id}
          disabled={!account.switchable}
          onClick={() => account.switchable && actions.switchTo(account.id)}
        >
          {account.switchable ? (isRec ? '推荐切换' : '切换') : '不可用'}
        </button>
        <button
          type="button"
          className="icon-btn"
          title="查询额度（会用少量额度）"
          data-action="refresh"
          data-id={account.id}
          aria-label={`查询${account.name}的额度（会用少量额度）`}
          aria-busy={account.checking || undefined}
          disabled={queryBusy}
          onClick={() => actions.query(account.id)}
        >
          <QuotaIcon name="refresh" size={13} />
        </button>
        <button
          type="button"
          className="icon-btn"
          aria-haspopup="menu"
          title="账号管理"
          data-action="menu"
          data-id={account.id}
          aria-label={`管理${account.name}`}
          onClick={(e) => actions.menu(account.id, e.currentTarget)}
        >
          <QuotaIcon name="more" size={15} />
        </button>
      </div>
    </div>
  );
}

export default function QuotaBoard({
  state,
  actions,
}: {
  state: BoardState;
  actions: BoardActions;
}) {
  const { current, items, recommended, total, terminalCount, view, sort, filter } = state;

  const limitingEntry = (() => {
    if (!current) return null;
    const candidates: Array<{ kind: '5 小时' | '7 天'; bucket: BoardBucket & { remaining: number } }> = [];
    if (!isUltraAccount(current) && hasReadableQuota(current.short)) {
      candidates.push({ kind: '5 小时', bucket: current.short });
    }
    if (hasReadableQuota(current.week)) {
      candidates.push({ kind: '7 天', bucket: current.week });
    }
    candidates.sort((a, b) => a.bucket.remaining - b.bucket.remaining);
    return candidates[0] || null;
  })();

  const showRecStrip = Boolean(
    recommended &&
      recommended.switchable &&
      (!current || recommended.id !== current.id) &&
      limitingEntry &&
      (limitingEntry.bucket.remaining <= 18),
  );

  const countText = filter === 'all' && items.length === total ? `${total}` : `${items.length} / ${total}`;

  const handleSortChange = (e: ChangeEvent<HTMLSelectElement>) => {
    actions.setSort(e.target.value as BoardState['sort']);
  };

  return (
    <div className="board">
      {actions.alerts}

      <div className="board-stage">
        <HeroSection current={current} terminalCount={terminalCount} actions={actions} />

        {showRecStrip && recommended && limitingEntry ? (
          <aside className="rec-strip">
            <QuotaIcon name="swap" size={16} />
            <span>
              当前{limitingEntry.kind}额度仅剩 <b>{clampPct(limitingEntry.bucket.remaining)}%</b>，推荐切到「
              <b>{recommended.name}</b>」
            </span>
            <button
              className="rec-link"
              data-action="switch"
              data-id={recommended.id}
              type="button"
              onClick={() => actions.switchTo(recommended.id)}
            >
              预览切换 <QuotaIcon name="arrow" size={14} />
            </button>
          </aside>
        ) : null}

        <div className="ctrl-bar">
          <div className="ctrl-left">
            <span className="sec-label">其他账号</span>
            <span className="sec-count">{countText}</span>
            <div className="seg-group" role="group" aria-label="筛选候选账号">
              {FILTER_OPTIONS.map((f) => {
                const isOn = filter === f.value;
                return (
                  <button
                    key={f.value}
                    type="button"
                    className={`seg-btn${isOn ? ' is-on' : ''}`}
                    data-action="filter"
                    data-value={f.value}
                    aria-pressed={isOn}
                    onClick={() => actions.setFilter(f.value)}
                  >
                    {f.label}
                  </button>
                );
              })}
            </div>
          </div>

          <div className="ctrl-right">
            <label className="sort-wrap">
              <span>排序</span>
              <select
                className="sort-select"
                data-control="sort"
                aria-label="候选账号排序"
                value={sort}
                onChange={handleSortChange}
              >
                {SORT_OPTIONS.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            </label>

            <div className="seg-group" role="group" aria-label="视图模式">
              <button
                type="button"
                className={`seg-btn${view === 'full' ? ' is-on' : ''}`}
                data-action="view"
                data-value="full"
                aria-pressed={view === 'full'}
                onClick={() => actions.setView('full')}
              >
                <QuotaIcon name="grid" size={13} />
                <span>标准视图</span>
              </button>
              <button
                type="button"
                className={`seg-btn${view === 'compact' ? ' is-on' : ''}`}
                data-action="view"
                data-value="compact"
                aria-pressed={view === 'compact'}
                onClick={() => actions.setView('compact')}
              >
                <QuotaIcon name="list" size={13} />
                <span>精简视图</span>
              </button>
            </div>
          </div>
        </div>

        <div id="candidate-list" className={`is-${view}`}>
          {view === 'full' ? (
            <section className="cand-grid" aria-label="其他账号卡片">
              {items.length === 0 ? (
                <div className="empty-box">
                  {total === 0 ? '尚未添加其他候选账号。' : '没有符合当前筛选条件的候选账号。'}
                </div>
              ) : (
                items.map((a) => {
                  const isRec = Boolean(
                    (a.recommended || (recommended && recommended.id === a.id)) &&
                      (!current || a.id !== current.id),
                  );
                  const recReason = a.reason || (isRec && recommended ? recommended.reason : '');
                  return (
                    <CandidateCard
                      key={a.id}
                      account={a}
                      isRec={isRec}
                      recReason={recReason}
                      actions={actions}
                    />
                  );
                })
              )}
            </section>
          ) : (
            <section className="cp-table" aria-label="精简账号额度对照表">
              <div className="cp-head" aria-hidden="true">
                <span>账号 / 套餐</span>
                <span>固定出口 / 本机端口</span>
                <span>5 小时剩余 · 重置</span>
                <span>7 天剩余 · 重置</span>
                <span style={{ textAlign: 'right' }}>操作</span>
              </div>
              {items.length === 0 ? (
                <div className="empty-box" style={{ border: 0, borderRadius: 0 }}>
                  {total === 0 ? '尚未添加其他候选账号。' : '没有符合当前筛选条件的候选账号。'}
                </div>
              ) : (
                items.map((a) => {
                  const isRec = Boolean(
                    (a.recommended || (recommended && recommended.id === a.id)) &&
                      (!current || a.id !== current.id),
                  );
                  const recReason = a.reason || (isRec && recommended ? recommended.reason : '');
                  return (
                    <CompactRow
                      key={a.id}
                      account={a}
                      isRec={isRec}
                      recReason={recReason}
                      actions={actions}
                    />
                  );
                })
              )}
            </section>
          )}
        </div>
      </div>
      {actions.footer}
    </div>
  );
}
