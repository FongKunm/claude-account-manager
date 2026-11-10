import { useEffect, useRef, useState } from 'react';
import type { Account, NormalLogin, OrdinaryAccount } from './api';
import { IconGlobe, IconKey, IconLogin, IconMore, IconPlus, IconRefresh, IconSwap, IconUser } from './icons';
import {
  IS_WINDOWS, bucketOf, buckets, clock, fableOf, initial, normalPlan, planOf, relTime, statusOf, subscriptionNote, switchState, until, usageTone,
  type BucketView, type Usable,
} from './util';

function PlanText({ a }: { a: Account }) {
  const p = planOf(a);
  return p ? <span className={`plan ${p.cls}`} title={p.auto ? '按额度自动判断' : '手动填写'}>{p.text}</span> : null;
}

function NormalPlanText({ n }: { n: NormalLogin }) {
  const p = normalPlan(n);
  return p ? <span className={`plan ${p.cls}`} title="账号资料里的套餐">{p.text}</span> : null;
}

function SubscriptionChip({ n }: { n: NormalLogin }) {
  const note = subscriptionNote(n);
  if (!note) return null;
  const via = n.billingType === 'apple_subscription' ? '（苹果订阅）' : '';
  return <span className="chip warn" title={`到期后不再是付费套餐${via}`}>{note}</span>;
}

function OverageChip({ a }: { a: Usable }) {
  const o = a.lastUsageCheck?.overage;
  if (!o) return null;
  return (
    <span className="chip" title={o.disabledReason ? `原因：${o.disabledReason}` : undefined}>
      额外用量{o.status === 'allowed' ? '已开' : '已关'}
    </span>
  );
}

export type Busy = { usage?: boolean; net?: boolean; switching?: boolean };

// ———— 小菜单 ————

export interface MenuItem {
  label: string;
  onSelect(): void;
  danger?: boolean;
  /** 危险操作二次确认时的文案 */
  confirm?: string;
  disabled?: boolean;
}

export function Menu({ items, label = '更多' }: { items: MenuItem[]; label?: string }) {
  const [open, setOpen] = useState(false);
  const [armed, setArmed] = useState<number | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = () => { setOpen(false); setArmed(null); };
    const onDown = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && close();
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close();
    window.addEventListener('mousedown', onDown);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('mousedown', onDown);
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);

  return (
    <div className="menu-wrap" ref={ref} onClick={(e) => e.stopPropagation()}>
      <button className="icon-btn sm" type="button" aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={() => { setOpen(!open); setArmed(null); }}>
        <IconMore />
      </button>
      {open && (
        <div className="pop" role="menu">
          {items.map((it, i) => (
            <button
              key={it.label}
              type="button"
              role="menuitem"
              disabled={it.disabled}
              className={`${it.danger ? 'danger' : ''}${armed === i ? ' armed' : ''}`}
              onClick={() => {
                if (it.confirm && armed !== i) return setArmed(i);
                setOpen(false);
                setArmed(null);
                it.onSelect();
              }}
            >
              {armed === i ? it.confirm : it.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// ———— 圆环 ————

function Ring({ b, label }: { b: BucketView | null; label: string }) {
  const r = 46;
  const c = 2 * Math.PI * r;
  const tone = !b ? '' : usageTone(b.pct);
  return (
    <div className="ring">
      <div className="ring-dial">
        <svg viewBox="0 0 108 108" aria-hidden="true">
          <circle className="ring-bg" cx="54" cy="54" r={r} />
          {b && b.pct > 0 && <circle className={`ring-fg ${tone}`} cx="54" cy="54" r={r} strokeDasharray={`${(c * b.pct) / 100} ${c}`} />}
        </svg>
        <div className="ring-num">
          {!b ? <span className="dim">—</span> : <><b className={tone}>{b.pct}</b><small>%</small></>}
        </div>
      </div>
      <div className="ring-label">{label}</div>
      <div className="ring-sub" title={b ? resetTitle(b) : undefined}>
        {!b ? '未查询' : resetNote(b)}
      </div>
    </div>
  );
}

/** 重置说明，大卡圆环和小卡进度条共用：有重置时间就倒计时；过了点不当 0%；没开始计时不编时间。 */
function resetNote(b: BucketView): string {
  if (b.stale) return '用了才开始计时';
  const left = until(b.resetsAt);
  if (left) return `${left}后重置`;
  return b.pct === 0 ? '用了才开始计时' : '—';
}

function resetTitle(b: BucketView): string | undefined {
  if (!b.resetsAt) return undefined;
  return b.stale ? `${clock(b.resetsAt)} 已重置，之后没再查到用量，按 0% 算` : `${clock(b.resetsAt)} 重置`;
}

/** reset：进度条下面多一行重置说明（小卡用）；行高固定，没查过也占位，卡片不跳。 */
function Bar({ b, label, reset = false }: { b: BucketView | null; label: string; reset?: boolean }) {
  const tone = !b ? '' : usageTone(b.pct);
  return (
    <div className={`bar-row${reset ? ' with-reset' : ''}`}>
      <span className="bar-label">{label}</span>
      <span className="bar-track"><i className={tone} style={{ width: `${b ? b.pct : 0}%` }} /></span>
      <span className={`bar-pct ${tone}`} title={b?.stale ? resetTitle(b) : undefined}>{!b ? '—' : `${b.pct}%`}</span>
      {reset && (
        <span className="bar-reset" title={b ? resetTitle(b) : undefined}>
          {b ? resetNote(b) : ''}
        </span>
      )}
    </div>
  );
}

// ———— 当前账号 ————

interface HeroProps {
  account: Account | null;
  normal: NormalLogin;
  /** 当前是换号台登记的普通账号（此时 account 为空） */
  ordinary?: OrdinaryAccount | null;
  rec: Account | null;
  busy: Busy;
  recBusy: boolean;
  menu: MenuItem[];
  /** 有任何手动查询在跑（后端一次只接一个） */
  usageLock: boolean;
  onRefresh(): void;
  onSwitch(a: Account): void;
}

export function CurrentHero({ account: a, normal, ordinary: o = null, rec, busy, recBusy, menu, usageLock, onRefresh, onSwitch }: HeroProps) {
  const src: Usable = a ?? { lastUsageCheck: (o ?? normal).lastUsageCheck };
  const five = bucketOf(src, '5h');
  const seven = bucketOf(src, '7d');
  const extras = buckets(src).filter((b) => b.key !== '5h' && b.key !== '7d');
  const net = a?.lastNetworkCheck;
  const usage = src.lastUsageCheck;
  const st = statusOf(src);
  const loggedIn = a || o || normal.email;

  // 底部一行：快用完时推荐切换；高度固定
  const low = !!five && five.pct >= 85;
  let tone = '';
  let strip: React.ReactNode = (
    <span className="strip-text">{a || o ? (IS_WINDOWS ? '切换时开着的对话换号后接着用' : '切换只影响新开的 claude，运行中的对话不变') : '插件、连接器全部可用；已存的账号随时可以切过去'}</span>
  );
  if (usage && !usage.ok && usage.error) {
    tone = usage.state === 'expired' ? 'warn' : 'bad';
    strip = <span className="strip-text">{usage.error}</span>;
  } else if (low && rec) {
    tone = five!.full ? 'bad' : 'warn';
    strip = (
      <>
        <span className="strip-text">5 小时额度{five!.full ? '已用完' : `只剩 ${100 - five!.pct}%`}，推荐切到「<b>{rec.name}</b>」</span>
        <button className="btn btn-white btn-sm" type="button" disabled={recBusy} onClick={() => onSwitch(rec)}>
          {recBusy ? <span className="spinner" /> : <IconSwap />}切换
        </button>
      </>
    );
  } else if (low) {
    tone = 'warn';
    strip = <span className="strip-text">额度快用完，暂时没有可切的账号</span>;
  }

  return (
    <section className="hero">
      <div className="hero-top">
        <span className="eyebrow"><i className="live" />当前</span>
        <span className="hero-when" title={usage?.source === 'native-probe' ? '手动查询的结果' : usage?.source === 'claude-code' ? '用 Claude Code 时顺带读到' : undefined}>{busy.usage ? '正在查询额度…' : usage ? `额度更新于 ${relTime(usage.checkedAt)}` : '还没查过额度'}</span>
        <button className={`icon-btn sm${busy.usage ? ' spin' : ''}`} type="button" title="通过 Claude Code 查询，会用少量额度" aria-label="查询额度" disabled={usageLock || !loggedIn} onClick={onRefresh}>
          <IconRefresh />
        </button>
        {menu.length > 0 && <Menu items={menu} />}
      </div>

      <div className="hero-body">
        {a ? (
          <div className="hero-id">
            <div className="avatar xl">{initial(a.name)}</div>
            <div className="hero-text">
              <div className="hero-name"><span title={a.name}>{a.name}</span><PlanText a={a} /></div>
              <div className="hero-sub mono">sk-…{a.tokenTail}{a.org ? ` · ${a.org.slice(0, 8)}` : ''}</div>
              <div className="hero-chips">
                <span className={`chip ${!net ? '' : net.ok ? 'ok' : 'bad'}`} title={net?.error ?? (net ? `检查于 ${relTime(net.checkedAt)}` : '未检查')}>
                  <IconGlobe />{a.expectedIp}
                </span>
                <span className={`chip ${st.tone}`}>{st.label}</span>
                <OverageChip a={src} />
              </div>
            </div>
          </div>
        ) : o ? (
          <div className="hero-id">
            <div className="avatar xl">{initial(o.name)}</div>
            <div className="hero-text">
              <div className="hero-name"><span title={o.name}>{o.name}</span><NormalPlanText n={o} /></div>
              <div className="hero-sub mono">{o.email ?? '普通登录'}</div>
              <div className="hero-chips">
                <span className={`chip ${usage?.ok ? 'ok' : ''}`} title="这个号的固定出口"><IconGlobe />{o.expectedIp}</span>
                {usage && <span className={`chip ${st.tone}`}>{st.label}</span>}
                <SubscriptionChip n={o} />
                <OverageChip a={src} />
                <span className="chip">普通登录</span>
              </div>
            </div>
          </div>
        ) : (
          <div className="hero-id">
            <div className="avatar xl icon"><IconUser /></div>
            <div className="hero-text">
              <div className="hero-name"><span>普通登录</span><NormalPlanText n={normal} /></div>
              <div className="hero-sub mono">{normal.email ?? '本机未登录'}</div>
              <div className="hero-chips">
                {normal.expectedIp && <span className={`chip ${usage?.ok ? 'ok' : ''}`} title="这个号的固定出口"><IconGlobe />{normal.expectedIp}</span>}
                {usage && <span className={`chip ${st.tone}`}>{st.label}</span>}
                <SubscriptionChip n={normal} />
                <OverageChip a={src} />
                {!usage && <span className="chip">本机 /login</span>}
              </div>
            </div>
          </div>
        )}

        <div className="hero-gauges">
          <Ring b={five} label="5 小时" />
          <Ring b={seven} label="7 天" />
          {extras.length > 0 && (
            <div className="hero-extras">
              {extras.slice(0, 3).map((b) => <Bar key={b.key} b={b} label={b.label} />)}
            </div>
          )}
        </div>
      </div>

      <div className={`hero-strip ${tone}`}>{strip}</div>
    </section>
  );
}

// ———— 切换到：账号卡片 ————

interface TileProps {
  a: Account;
  /** 和本机普通登录是同一个号时，那个号的邮箱 */
  sameAsNormal: string | null;
  recommended: boolean;
  busy: Busy;
  lock: boolean;
  /** 有任何手动查询在跑（后端一次只接一个） */
  usageLock: boolean;
  menu: MenuItem[];
  onUsage(): void;
  onSwitch(): void;
}

export function AccountTile({ a, sameAsNormal, recommended, busy, lock, usageLock, menu, onUsage, onSwitch }: TileProps) {
  const s = switchState(a);
  const five = bucketOf(a, '5h');
  const seven = bucketOf(a, '7d');
  const fable = fableOf(a);
  const usage = a.lastUsageCheck;
  const st = statusOf(a);

  let sub: { text: string; tone: string } = { text: usage ? `更新于 ${relTime(usage.checkedAt)}` : '还没查过额度', tone: '' };
  if (busy.usage) sub = { text: '正在查询…', tone: '' };
  else if (!s.ok) sub = { text: s.kind === 'wait' ? s.reason : st.label, tone: s.kind === 'wait' ? 'bad' : 'bad' };
  else if (usage && !usage.ok) sub = { text: usage.error ?? st.label, tone: 'warn' };

  const cls = ['tile', recommended && 'rec', !s.ok && (s.kind === 'wait' ? 'wait' : 'broken')].filter(Boolean).join(' ');

  return (
    <article className={cls}>
      <header className="tile-head">
        <div className="avatar">{initial(a.name)}</div>
        <div className="tile-id">
          <div className="tile-name"><span title={a.name}>{a.name}</span><PlanText a={a} />{recommended && <span className="tag rec">推荐</span>}</div>
          <div className="tile-sub mono">
            {sameAsNormal && sameAsNormal.toLowerCase() !== a.name.trim().toLowerCase()
              ? <span title={`和普通登录是同一个号，额度共用 · sk-…${a.tokenTail}`}>{sameAsNormal}</span>
              : <>
                  sk-…{a.tokenTail}
                  {sameAsNormal !== null && <span className="same" title="和普通登录是同一个号，额度共用">同普通登录</span>}
                </>}
          </div>
        </div>
        <Menu items={menu} />
      </header>

      <div className="tile-meters">
        <Bar b={five} label="5 小时" reset />
        <Bar b={seven} label="7 天" reset />
        {fable ? <Bar b={fable} label="7 天 Fable" reset /> : <div className="bar-row with-reset placeholder" />}
      </div>

      <footer className="tile-foot">
        <span className={`tile-note ${sub.tone}`} title={sub.text}>{sub.text}</span>
        <button className={`icon-btn sm${busy.usage ? ' spin' : ''}`} type="button" title="通过 Claude Code 查询，会用少量额度" aria-label="查询额度" disabled={usageLock || st.label === '令牌无效'} onClick={onUsage}>
          <IconRefresh />
        </button>
        <button
          className={`btn btn-sm${recommended ? ' btn-white' : ''}`}
          type="button"
          disabled={!s.ok || lock || busy.switching}
          onClick={onSwitch}
          title={!s.ok ? s.reason : undefined}
        >
          {busy.switching ? <span className="spinner" /> : <IconSwap />}切换
        </button>
      </footer>
    </article>
  );
}

// ———— 切换到：普通登录 ————

export function NormalTile({ normal, busy, usageBusy, lock, usageLock, menu, onUsage, onSwitch }: {
  normal: NormalLogin; busy: boolean; usageBusy: boolean; lock: boolean; usageLock: boolean; menu: MenuItem[]; onUsage(): void; onSwitch(): void;
}) {
  const src: Usable = { lastUsageCheck: normal.lastUsageCheck };
  const usage = normal.lastUsageCheck;
  const fable = fableOf(src);
  const sub = subscriptionNote(normal);
  let note: { text: string; tone: string } = { text: usage ? `更新于 ${relTime(usage.checkedAt)}` : '插件、连接器全部可用', tone: '' };
  if (usageBusy) note = { text: '正在查询…', tone: '' };
  else if (usage && !usage.ok) note = { text: usage.error ?? statusOf(src).label, tone: 'warn' };
  else if (sub) note = { text: sub, tone: 'warn' };
  return (
    <article className="tile normal">
      <header className="tile-head">
        <div className="avatar icon"><IconUser /></div>
        <div className="tile-id">
          <div className="tile-name"><span>普通登录</span><NormalPlanText n={normal} /></div>
          <div className="tile-sub mono">{normal.email ?? '本机未登录'}</div>
        </div>
        {menu.length > 0 && <Menu items={menu} />}
      </header>
      <div className="tile-meters">
        <Bar b={bucketOf(src, '5h')} label="5 小时" reset />
        <Bar b={bucketOf(src, '7d')} label="7 天" reset />
        {fable ? <Bar b={fable} label="7 天 Fable" reset /> : <div className="bar-row with-reset placeholder" />}
      </div>
      <footer className="tile-foot">
        <span className={`tile-note ${note.tone}`} title={note.text}>{note.text}</span>
        <button className={`icon-btn sm${usageBusy ? ' spin' : ''}`} type="button" title="通过 Claude Code 查询，会用少量额度" aria-label="查询额度" disabled={usageLock || !normal.email} onClick={onUsage}>
          <IconRefresh />
        </button>
        <button className="btn btn-sm" type="button" disabled={lock || busy || !normal.email} onClick={onSwitch} title={!normal.email ? '本机没有 /login 登录' : undefined}>
          {busy ? <span className="spinner" /> : <IconSwap />}切换
        </button>
      </footer>
    </article>
  );
}

// ———— 切换到：换号台登记的普通账号（没有导出、删除，只能查额度和切换） ————

export function OrdinaryTile({ o, busy, usageBusy, lock, usageLock, onUsage, onSwitch }: {
  o: OrdinaryAccount; busy: boolean; usageBusy: boolean; lock: boolean; usageLock: boolean; onUsage(): void; onSwitch(): void;
}) {
  const src: Usable = { lastUsageCheck: o.lastUsageCheck };
  const usage = o.lastUsageCheck;
  const fable = fableOf(src);
  const sub = subscriptionNote(o);
  let note: { text: string; tone: string } = { text: usage ? `更新于 ${relTime(usage.checkedAt)}` : '还没查过额度', tone: '' };
  if (usageBusy) note = { text: '正在查询…', tone: '' };
  else if (usage && !usage.ok) note = { text: usage.error ?? statusOf(src).label, tone: 'warn' };
  else if (sub) note = { text: sub, tone: 'warn' };
  return (
    <article className="tile ordinary">
      <header className="tile-head">
        <div className="avatar">{initial(o.name)}</div>
        <div className="tile-id">
          <div className="tile-name"><span title={o.name}>{o.name}</span><NormalPlanText n={o} /></div>
          <div className="tile-sub mono" title={`固定出口 ${o.expectedIp}`}>{o.email ?? '普通登录'} · {o.expectedIp}</div>
        </div>
      </header>
      <div className="tile-meters">
        <Bar b={bucketOf(src, '5h')} label="5 小时" reset />
        <Bar b={bucketOf(src, '7d')} label="7 天" reset />
        {fable ? <Bar b={fable} label="7 天 Fable" reset /> : <div className="bar-row with-reset placeholder" />}
      </div>
      <footer className="tile-foot">
        <span className={`tile-note ${note.tone}`} title={note.text}>{note.text}</span>
        <button className={`icon-btn sm${usageBusy ? ' spin' : ''}`} type="button" title="通过 Claude Code 查询，会用少量额度" aria-label="查询额度" disabled={usageLock} onClick={onUsage}>
          <IconRefresh />
        </button>
        <button className="btn btn-sm" type="button" disabled={lock || busy} onClick={onSwitch}>
          {busy ? <span className="spinner" /> : <IconSwap />}切换
        </button>
      </footer>
    </article>
  );
}

export function AddTile({ onAdd }: { onAdd(): void }) {
  return (
    <button className="tile add-tile" type="button" onClick={onAdd}>
      <span className="add-mark"><IconPlus /></span>
      <span className="add-title">添加令牌号</span>
      <span className="add-sub"><IconKey />粘贴 setup token</span>
    </button>
  );
}

export function AddNormalTile({ onAdd }: { onAdd(): void }) {
  return (
    <button className="tile add-tile" type="button" onClick={onAdd}>
      <span className="add-mark"><IconPlus /></span>
      <span className="add-title">添加普通账号</span>
      <span className="add-sub"><IconLogin />走官方登录</span>
    </button>
  );
}
