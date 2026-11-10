import { useEffect, useRef, useState, type FormEvent } from 'react';
import { Shell } from './Dialogs';
import { NormalLoginDialog } from './NormalLoginDialog';
import { api, errorText, type Account, type AppState, type Plan } from './api';
import { QuotaIcon } from './QuotaIcon';
import type { BoardAccount } from './quotaView';
import { IS_WINDOWS, portOf, sameIp, samePort } from './util';

type LoginMethod = '账号登录' | 'Setup Token';

interface NormalLoginInitial {
  name: string;
  note: string;
  plan: Plan;
  proxy: string;
  expectedIp: string;
  sourceTokenId?: string;
}

interface ExitObservation {
  proxy: string;
  ip: string;
}

const IPV4_RE = /^(\d{1,3}\.){3}\d{1,3}$/;

const PLAN_OPTIONS: ReadonlyArray<{ value: Plan; label: string }> = [
  { value: '', label: '未指定' },
  { value: 'pro', label: 'Pro' },
  { value: 'max5', label: 'Max' },
  { value: 'max20', label: 'Ultra' },
];

function displayPlanToCode(planLabel: string | undefined): Plan {
  const norm = (planLabel || '').trim().toLowerCase();
  if (norm === 'pro') return 'pro';
  if (norm === 'ultra' || norm.includes('20')) return 'max20';
  if (norm.startsWith('max')) return 'max5';
  return '';
}

function isValidIpFormat(raw: string): boolean {
  const trimmed = raw.trim();
  if (!trimmed) return false;
  if (IPV4_RE.test(trimmed)) {
    return trimmed.split('.').every((part) => {
      const n = Number(part);
      return n >= 0 && n <= 255;
    });
  }
  return trimmed.includes(':');
}

function getEligibleTokenSources(state: AppState): Account[] {
  const ordinaryList = state.ordinaryAccounts ?? [];
  return state.accounts.filter((card) => {
    if (!card.org) return false;
    return !ordinaryList.some((ord) => ord.org && ord.org === card.org);
  });
}

function findRouteConflict(
  state: AppState,
  proxy: string,
  ip: string,
  options: {
    editAccount?: BoardAccount;
    sourceTokenId?: string;
    editingType: LoginMethod;
  },
): { error?: string; warn?: string } {
  const p = proxy.trim();
  const i = ip.trim();
  const { editAccount, sourceTokenId, editingType } = options;

  const pairedId = editAccount && (editingType === '账号登录'
    ? state.ordinaryAccounts?.find(o => o.id === editAccount.id)?.sourceTokenId
    : state.ordinaryAccounts?.find(o => o.sourceTokenId === editAccount.id)?.id);

  for (const card of state.accounts) {
    if (editAccount && card.id === editAccount.id) continue;
    if (sourceTokenId && card.id === sourceTokenId) continue;
    if (card.id === pairedId) continue;

    if (p && samePort(card.proxy, p)) {
      if (!editAccount && editingType === '账号登录') {
        return {
          error: `这个端口已绑定到令牌卡「${card.name}」。若要登记同一个号，请在「账号来源」中选择该令牌卡。`,
        };
      }
      return { error: `这个端口已绑定到令牌卡「${card.name}」，请使用独立代理端口。` };
    }

    if (i && card.expectedIp && sameIp(card.expectedIp, i)) {
      if (!editAccount && editingType === '账号登录') {
        return {
          error: `出口 ${i} 已绑定到令牌卡「${card.name}」。若要登记同一个号，请在「账号来源」中选择该令牌卡。`,
        };
      }
      return { error: `出口 ${i} 已绑定到令牌卡「${card.name}」，每个账号须使用独立固定出口。` };
    }
  }

  for (const ord of state.ordinaryAccounts ?? []) {
    if (editAccount && ord.id === editAccount.id) continue;
    if (ord.id === pairedId) continue;
    if (p && ord.proxy && samePort(ord.proxy, p)) {
      return { error: '这个端口已绑定到「' + ord.name + '」，请使用独立代理端口。' };
    }

    if (i && ord.expectedIp && sameIp(ord.expectedIp, i)) {
      return { error: `出口 ${i} 已绑定到「${ord.name}」，每个账号须使用独立固定出口。` };
    }
  }

  if (
    !editAccount &&
    state.normal.email &&
    ((p && state.defaultProxy && samePort(state.defaultProxy, p)) ||
      (i && state.normal.expectedIp && sameIp(state.normal.expectedIp, i)))
  ) {
    return { warn: '本机账号登录也走这个出口，仅在确认为同一个账号时可继续。' };
  }

  return {};
}

export function AccountForm({
  state,
  account,
  onClose,
  onDone,
}: {
  state: AppState;
  account?: BoardAccount;
  onClose(): void;
  onDone(msg: string): void;
}) {
  const isEdit = Boolean(account);
  const originalProxy = account?.proxy.trim() ?? '';
  const originalIp = account?.ip.trim() ?? '';

  const [editingType, setEditingType] = useState<LoginMethod>(account?.method ?? (IS_WINDOWS ? '账号登录' : 'Setup Token'));
  const [sameSource, setSameSource] = useState<string>('');
  const [name, setName] = useState<string>(account?.name ?? '');
  const [plan, setPlan] = useState<Plan>(() => displayPlanToCode(account?.plan));
  const [note, setNote] = useState<string>(account?.note ?? '');
  const [proxy, setProxy] = useState<string>(account?.proxy ?? '');
  const [ip, setIp] = useState<string>(account?.ip ?? '');
  const [token, setToken] = useState<string>('');
  const [confirmPair, setConfirmPair] = useState<boolean>(false);

  const [observation, setObservation] = useState<ExitObservation | null>(null);
  const [detecting, setDetecting] = useState<boolean>(false);
  const [saving, setSaving] = useState<boolean>(false);
  const [error, setError] = useState<string>('');
  const [authDraft, setAuthDraft] = useState<NormalLoginInitial | null>(null);

  const aliveRef = useRef<boolean>(true);
  const detectSeqRef = useRef<number>(0);
  const nameInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  const availableSources = !isEdit && editingType === '账号登录' ? getEligibleTokenSources(state) : [];
  const sourceCard =
    !isEdit && editingType === '账号登录' && sameSource
      ? state.accounts.find((c) => c.id === sameSource) ?? null
      : null;

  const effectiveProxy = sourceCard ? sourceCard.proxy : proxy;
  const effectiveIp = sourceCard
    ? sourceCard.expectedIp
    : isEdit && originalIp
      ? originalIp
      : ip;

  const hasPair = Boolean(account?.pairName);

  const routeChanged = isEdit
    ? effectiveProxy.trim() !== originalProxy ||
      Boolean(originalIp && !sameIp(effectiveIp.trim(), originalIp)) ||
      Boolean(!originalIp && effectiveIp.trim() !== '')
    : true;

  const conflictPreview = findRouteConflict(state, effectiveProxy, effectiveIp, {
    editAccount: account,
    sourceTokenId: sourceCard?.id,
    editingType,
  });

  function invalidateObservation() {
    detectSeqRef.current += 1;
    setDetecting(false);
    setObservation(null);
  }

  function handleSwitchType(nextType: LoginMethod) {
    if (nextType === editingType || saving) return;
    const wasLockedToSource = Boolean(sourceCard);
    setEditingType(nextType);
    setSameSource('');
    setToken('');
    setError('');
    invalidateObservation();
    if (wasLockedToSource) {
      setProxy('');
      setIp('');
    }
  }

  function handleSourceChange(nextSourceId: string) {
    setSameSource(nextSourceId);
    setError('');
    invalidateObservation();

    if (!nextSourceId) {
      setProxy('');
      setIp('');
      return;
    }

    const picked = state.accounts.find((c) => c.id === nextSourceId);
    if (picked) {
      setProxy(picked.proxy);
      setIp(picked.expectedIp);
      if (!name.trim()) {
        setName(picked.name);
      }
    }
  }

  async function handleDetectExit() {
    if (detecting || saving) return;
    const trimmedProxy = effectiveProxy.trim();
    const port = portOf(trimmedProxy);
    if (!port) {
      setError('请先填写带端口的本机代理，例如 http://127.0.0.1:31111。');
      return;
    }

    setError('');
    setDetecting(true);
    const seq = ++detectSeqRef.current;

    try {
      const detectedRaw = await api.detectExit(trimmedProxy);
      if (!aliveRef.current || seq !== detectSeqRef.current) return;

      const detectedIp = detectedRaw.ip.trim();
      setObservation({ proxy: trimmedProxy, ip: detectedIp });

      // 编辑已有固定出口或复用原令牌卡时，严禁把测到的新 IP 覆盖原固定出口
      const lockedIp = sourceCard ? sourceCard.expectedIp.trim() : isEdit ? originalIp : '';
      let targetIp = lockedIp || ip.trim();

      if (!lockedIp && !targetIp) {
        setIp(detectedIp);
        targetIp = detectedIp;
      }

      if (targetIp && !sameIp(targetIp, detectedIp)) {
        setError(
          isEdit || sourceCard
            ? `检测到的出口（${detectedIp}）与该账号已绑定的固定 IP（${targetIp}）不符，请更换代理端口。`
            : `检测到的出口（${detectedIp}）与填写的固定出口 IP（${targetIp}）不符，请核对。`,
        );
      }
    } catch (err) {
      if (!aliveRef.current || seq !== detectSeqRef.current) return;
      setObservation(null);
      setError(errorText(err));
    } finally {
      if (aliveRef.current && seq === detectSeqRef.current) {
        setDetecting(false);
      }
    }
  }

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    if (saving || detecting) return;

    const trimmedName = name.trim();
    const trimmedNote = note.trim();
    const trimmedProxy = effectiveProxy.trim();
    const trimmedIp = effectiveIp.trim();

    if (!trimmedName) {
      setError('请填写账号名称。');
      nameInputRef.current?.focus();
      return;
    }

    // 编辑模式：仅改名称 / 备注 / 套餐，未动代理与出口时完全离线保存
    if (isEdit && account && !routeChanged) {
      setError('');
      setSaving(true);
      try {
        await api.editCard({
          id: account.id,
          name: trimmedName,
          note: trimmedNote,
          plan,
          proxy: originalProxy,
          expectedIp: originalIp,
          updatePair: false,
        });
        if (!aliveRef.current) return;
        onDone(`已保存「${trimmedName}」`);
      } catch (err) {
        if (!aliveRef.current) return;
        setError(errorText(err));
      } finally {
        if (aliveRef.current) setSaving(false);
      }
      return;
    }

    const port = portOf(trimmedProxy);
    if (!port) {
      setError('请填写带端口的本机代理，例如 http://127.0.0.1:31111。');
      return;
    }

    if (!isValidIpFormat(trimmedIp)) {
      setError('请填写有效的固定出口 IP，或点击「检测出口」。');
      return;
    }

    if (sourceCard) {
      if (!samePort(trimmedProxy, sourceCard.proxy) || !sameIp(trimmedIp, sourceCard.expectedIp)) {
        setError('同一账号的两张卡必须使用完全相同的本机代理端口与固定出口 IP。');
        return;
      }
    }

    if (isEdit && originalIp && !sameIp(trimmedIp, originalIp)) {
      setError('新代理须保留该账号原有的固定出口 IP。');
      return;
    }

    if (conflictPreview.error) {
      setError(conflictPreview.error);
      return;
    }

    const verifiedForCurrentProxy = observation && observation.proxy === trimmedProxy;

    if (isEdit && routeChanged) {
      if (!verifiedForCurrentProxy) {
        setError('更换本机代理端口后，请先点击「检测出口」核对出口仍与原固定 IP 一致。');
        return;
      }
      if (!sameIp(observation.ip, trimmedIp)) {
        setError(
          `检测到的出口（${observation.ip}）与该账号固定出口（${trimmedIp}）不符，无法保存。`,
        );
        return;
      }
      if (hasPair && !confirmPair) {
        setError('请勾选确认同一账号的两张卡一起改用这个代理端口。');
        return;
      }

      setError('');
      setSaving(true);
      try {
        await api.editCard({
          id: account!.id,
          name: trimmedName,
          note: trimmedNote,
          plan,
          proxy: trimmedProxy,
          expectedIp: originalIp || trimmedIp,
          updatePair: hasPair && confirmPair,
        });
        if (!aliveRef.current) return;
        onDone(`已保存「${trimmedName}」，并核对固定出口`);
      } catch (err) {
        if (!aliveRef.current) return;
        setError(errorText(err));
      } finally {
        if (aliveRef.current) setSaving(false);
      }
      return;
    }

    // 新增模式：要求先主动点击「检测出口」核对一致
    if (!verifiedForCurrentProxy) {
      setError('请先点击「检测出口」，确认本机代理可用且与固定出口 IP 一致。');
      return;
    }
    if (!sameIp(observation.ip, trimmedIp)) {
      setError(
        sourceCard
          ? `当前端口检测到的出口（${observation.ip}）与原令牌卡固定 IP（${trimmedIp}）不符。`
          : `检测到的出口（${observation.ip}）与填写的固定出口 IP（${trimmedIp}）不符，请核对。`,
      );
      return;
    }

    if (editingType === '账号登录') {
      setError('');
      setAuthDraft({
        name: trimmedName,
        note: trimmedNote,
        plan,
        proxy: trimmedProxy,
        expectedIp: trimmedIp,
        sourceTokenId: sourceCard?.id,
      });
      return;
    }

    const rawToken = token.trim();
    if (!rawToken) {
      setError('请填写 Setup Token。');
      return;
    }

    setError('');
    setSaving(true);
    try {
      await api.addAccount({
        name: trimmedName,
        note: trimmedNote,
        plan,
        proxy: trimmedProxy,
        expectedIp: trimmedIp,
        token: rawToken,
      });
      if (!aliveRef.current) return;
      setToken('');
      onDone(`已添加「${trimmedName}」`);
    } catch (err) {
      if (!aliveRef.current) return;
      setError(errorText(err));
    } finally {
      if (aliveRef.current) setSaving(false);
    }
  }

  if (!isEdit && authDraft) {
    return (
      <NormalLoginDialog
        state={state}
        initial={authDraft}
        autoBegin
        onBack={() => setAuthDraft(null)}
        onClose={onClose}
        onDone={onDone}
      />
    );
  }

  const isOrdinary = editingType === '账号登录';
  const proxyLocked = Boolean(sourceCard);
  const ipLocked = Boolean(sourceCard) || Boolean(isEdit && originalIp);

  const routeResultText = (() => {
    if (detecting) {
      return '正在通过本机代理检测出口 IP…';
    }
    if (observation && observation.proxy === effectiveProxy.trim()) {
      const target = effectiveIp.trim();
      const matched = Boolean(target && sameIp(observation.ip, target));
      return `检测出口 ${observation.ip}${target ? (matched ? ' · 一致' : ' · 与固定出口不符') : ''}`;
    }
    if (sourceCard) {
      return `已复用令牌卡「${sourceCard.name}」的端口与固定出口，请点「检测出口」核对。`;
    }
    if (isEdit && originalIp) {
      return routeChanged
        ? '已更改代理端口，保存前请点「检测出口」确认仍为原固定出口。'
        : '仅修改名称、套餐或备注可直接离线保存；更换端口须匹配原固定出口。';
    }
    return '请点击「检测出口」核对本机专用端口的出口 IP。';
  })();

  const submitLabel = isEdit
    ? saving
      ? '正在保存…'
      : '保存修改'
    : isOrdinary
      ? '下一步 · 账号授权'
      : saving
        ? '正在添加…'
        : '添加令牌账号';

  return (
    <Shell
      title={isEdit ? '编辑账号' : '添加账号'}
      sub={isEdit ? undefined : '填写名称和专用代理，核对固定出口后登记账号。'}
      busy={saving}
      onClose={onClose}
      onSubmit={handleSubmit}
      foot={
        <>
          <button type="button" className="ui-btn" disabled={saving} onClick={onClose}>
            取消
          </button>
          <button type="submit" className="ui-btn primary" disabled={saving || detecting}>
            {submitLabel}
          </button>
        </>
      }
    >
      {!isEdit ? (
        <div className="form-types" role="group" aria-label="登录方式">
          {((IS_WINDOWS ? ['账号登录', 'Setup Token'] : ['Setup Token']) as LoginMethod[]).map((t) => (
            <button
              key={t}
              type="button"
              className="ui-btn"
              data-type={t}
              aria-pressed={editingType === t}
              disabled={saving || detecting}
              onClick={() => handleSwitchType(t)}
            >
              <QuotaIcon name={t === '账号登录' ? 'user' : 'key'} size={15} /> {t}
            </button>
          ))}
        </div>
      ) : (
        <div className="edit-info">
          <span className="ui-tag">{account!.method}</span>
          {account!.email ? <code>{account!.email}</code> : null}
        </div>
      )}

      {!isEdit && isOrdinary && availableSources.length > 0 ? (
        <label className="field route-source" htmlFor="f-source">
          <span>账号来源</span>
          <select
            id="f-source"
            value={sameSource}
            disabled={saving || detecting}
            onChange={(e) => handleSourceChange(e.target.value)}
          >
            <option value="">新账号</option>
            {availableSources.map((s) => (
              <option key={s.id} value={s.id}>
                与令牌卡「{s.name}」是同一个号
              </option>
            ))}
          </select>
        </label>
      ) : null}

      <fieldset className="fieldgroup">
        <legend>账号资料</legend>
        <div className="form-grid">
          <label className="field" htmlFor="f-name">
            <span>账号名称</span>
            <input
              ref={nameInputRef}
              id="f-name"
              value={name}
              required
              maxLength={48}
              disabled={saving}
              placeholder="给账号起一个好认的名字"
              onChange={(e) => {
                setName(e.target.value);
                setError('');
              }}
            />
          </label>

          <label className="field" htmlFor="f-plan">
            <span>套餐</span>
            <select
              id="f-plan"
              value={plan}
              disabled={saving}
              onChange={(e) => {
                setPlan(e.target.value as Plan);
                setError('');
              }}
            >
              {PLAN_OPTIONS.map((opt) => (
                <option key={opt.value || 'unset'} value={opt.value}>
                  {opt.label}
                </option>
              ))}
            </select>
          </label>
        </div>

        <label className="field" htmlFor="f-note">
          <span>
            备注 <small>可选</small>
          </span>
          <input
            id="f-note"
            value={note}
            maxLength={140}
            disabled={saving}
            placeholder="用途、订阅时间或其他备忘"
            onChange={(e) => {
              setNote(e.target.value);
              setError('');
            }}
          />
        </label>

        {!isEdit && !isOrdinary ? (
          <div className="token-field">
            <label className="field" htmlFor="f-token">
              <span>Setup Token</span>
              <input
                id="f-token"
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={token}
                disabled={saving}
                placeholder="粘贴 claude setup-token 生成的令牌"
                onChange={(e) => {
                  setToken(e.target.value);
                  setError('');
                }}
              />
            </label>
          </div>
        ) : null}
      </fieldset>

      <fieldset className="fieldgroup">
        <legend>代理与出口</legend>
        <div className="form-grid">
          <label className="field" htmlFor="f-proxy">
            <span>本机代理</span>
            <input
              id="f-proxy"
              value={effectiveProxy}
              readOnly={proxyLocked}
              disabled={saving}
              maxLength={96}
              spellCheck={false}
              placeholder="http://127.0.0.1:31111"
              onChange={(e) => {
                if (proxyLocked) return;
                setProxy(e.target.value);
                setConfirmPair(false);
                setError('');
                invalidateObservation();
              }}
            />
          </label>

          <label className="field" htmlFor="f-ip">
            <span>固定出口 IP</span>
            <input
              id="f-ip"
              value={effectiveIp}
              readOnly={ipLocked}
              disabled={saving}
              maxLength={64}
              spellCheck={false}
              placeholder="填写或检测"
              onChange={(e) => {
                if (ipLocked) return;
                setIp(e.target.value);
                setConfirmPair(false);
                setError('');
                invalidateObservation();
              }}
            />
          </label>
        </div>

        <div className="route-check">
          <button
            type="button"
            className="ui-btn minor"
            disabled={saving || detecting || !effectiveProxy.trim()}
            onClick={handleDetectExit}
          >
            <QuotaIcon name="globe" size={14} /> {detecting ? '正在检测…' : '检测出口'}
          </button>
          <span id="route-result" aria-live="polite">
            {routeResultText}
          </span>
        </div>

        {isEdit && hasPair ? (
          <label className="consent" id="route-consent" hidden={!routeChanged}>
            <input
              id="confirm-pair"
              type="checkbox"
              checked={confirmPair}
              disabled={saving}
              onChange={(e) => {
                setConfirmPair(e.target.checked);
                setError('');
              }}
            />{' '}
            {account?.pairName
              ? `同一账号关联的「${account.pairName}」一起改用这个代理端口`
              : '同一账号的两张卡一起改用这个代理端口'}
          </label>
        ) : null}
      </fieldset>

      {!isEdit && isOrdinary ? (
        <p className="helper">
          {IS_WINDOWS
            ? '由 Claude Code 官方登录完成授权；请先让浏览器走该账号出口，下一步也提供独立代理浏览器核对指引。'
            : '由 Claude Code 官方登录完成授权。请先让浏览器走这个账号的出口。'}
        </p>
      ) : null}

      {conflictPreview.warn && !error ? <p className="helper">{conflictPreview.warn}</p> : null}

      <p className="ui-error" role="alert" id="form-error" hidden={!error}>
        {error}
      </p>
    </Shell>
  );
}
