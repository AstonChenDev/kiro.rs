import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import {
  TIERS, TIER_NAMES, getPolicies, savePolicies, getTraffic, saveCredentialTraffic,
  startBalanceRefresh, getBalanceRefresh, restoreCredentialTrafficLimits,
  type Policies, type CredentialTraffic, type TrafficStatus, type AccountTier, type RefreshJob,
} from '@/api/traffic'
import { extractErrorMessage } from '@/lib/utils'
import { BALANCE_REFRESH_KEY, createBalanceRefreshController } from '@/lib/balance-refresh'

const TrafficContext = createContext<{
  rows: TrafficStatus[]; stale: boolean; errors: Record<string, string>; updatedAt: number
  refreshJob?: RefreshJob; refreshPending: boolean; refreshBalances: () => void
}>({ rows: [], stale: true, errors: {}, updatedAt: 0, refreshPending: false, refreshBalances: () => {} })
export const useTrafficState = () => useContext(TrafficContext)

export function TrafficProvider({ children }: { children: ReactNode }) {
  const client = useQueryClient()
  const [refreshController] = useState(() => createBalanceRefreshController(client, {
    start: startBalanceRefresh, read: getBalanceRefresh,
  }))
  const { mutate: refreshBalances, isPending: refreshPending } = useMutation({
    mutationFn: refreshController.start,
    onError: error => toast.error(`刷新额度失败：${extractErrorMessage(error)}`),
  })
  const [visible, setVisible] = useState(!document.hidden)
  useEffect(() => {
    const onVisibility = () => setVisible(!document.hidden)
    document.addEventListener('visibilitychange', onVisibility)
    return () => document.removeEventListener('visibilitychange', onVisibility)
  }, [])
  const traffic = useQuery({ queryKey: ['credential-traffic'], queryFn: getTraffic, refetchInterval: 2000, enabled: visible, retry: false })
  const job = useQuery({ queryKey: BALANCE_REFRESH_KEY, queryFn: refreshController.read, enabled: visible,
    refetchInterval: q => q.state.data?.running ? 2000 : false })
  useEffect(() => {
    refreshBalances()
  }, [refreshBalances])
  useEffect(() => {
    if (job.data) {
      client.invalidateQueries({ queryKey: ['credentials'] })
      client.invalidateQueries({ queryKey: ['stats'] })
    }
  }, [job.data?.completed, job.data?.finishedAt, client])
  return <TrafficContext.Provider value={{ rows: traffic.data ?? [], stale: traffic.isError || !traffic.data || !visible || Date.now() - traffic.dataUpdatedAt > 6000,
    updatedAt: traffic.dataUpdatedAt,
    refreshJob: job.data, refreshPending, refreshBalances,
    errors: job.data?.errors ?? {} }}>{children}</TrafficContext.Provider>
}

const numberValue = (text: string): number | null => text === '' ? null : Number(text)
const cap = (value: number) => value === 0 ? '不限' : String(value)
const origin = (value: string) => value === 'credential' ? '单账号' : value === 'tier' ? '类型策略' : '全局默认'
const valid = (value: number | null) => value === null || (Number.isInteger(value) && value >= 0 && value <= 100000)

function LimitInput({ label, value, onChange }: { label: string; value: number | null; onChange: (value: number | null) => void }) {
  return <Input aria-label={label} type="number" min={0} max={100000} step={1} value={value ?? ''}
    placeholder="继承" onChange={event => onChange(numberValue(event.target.value))} className="h-8 min-w-0" />
}

export function TrafficPoliciesPanel() {
  const client = useQueryClient()
  const { rows, refreshJob, refreshPending, refreshBalances } = useContext(TrafficContext)
  const query = useQuery({ queryKey: ['traffic-policies'], queryFn: getPolicies })
  const [draft, setDraft] = useState<Policies | null>(null)
  const [dirty, setDirty] = useState(false)
  const [original, setOriginal] = useState<Policies | null>(null)
  useEffect(() => { if (query.data && !dirty) { setDraft(query.data); setOriginal(query.data) } }, [query.data, dirty])
  const save = useMutation({ mutationFn: savePolicies, onSuccess: data => {
    client.setQueryData(['traffic-policies'], data); setDirty(false)
    client.invalidateQueries({ queryKey: ['credential-traffic'] }); toast.success('类型策略已保存，继承账号及后续新增账号立即采用该策略')
  }, onError: e => toast.error(extractErrorMessage(e)) })
  const edit = (next: Policies) => { setDraft(next); setDirty(true) }
  return <section className="mb-5 rounded-lg border bg-card p-4" aria-label="账号类型策略">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <div><h2 className="font-semibold text-sm">账号类型策略与额度刷新</h2>
        <p className="mt-1 text-xs text-muted-foreground">每次进入自动刷新额度与用量。类型策略作用于每个账号；单账号设置优先。</p></div>
      <Button size="sm" variant="outline" disabled={refreshPending || refreshJob?.running} onClick={refreshBalances}>刷新全部额度</Button>
    </div>
    <p className="mt-2 text-xs" role="status">{refreshPending ? '正在启动额度刷新…' : refreshJob?.running
      ? `额度刷新中 ${refreshJob.completed}/${refreshJob.total}，失败 ${refreshJob.failed}，跳过 ${refreshJob.skipped}`
      : refreshJob?.finishedAt ? `最近刷新 ${new Date(refreshJob.finishedAt).toLocaleTimeString()} · 成功 ${refreshJob.completed - refreshJob.failed - refreshJob.skipped} · 失败 ${refreshJob.failed} · 跳过 ${refreshJob.skipped}` : '等待额度刷新'}</p>
    {query.isError && <p className="mt-2 text-xs text-destructive">策略读取失败，请重试。<Button variant="link" onClick={() => query.refetch()}>重试</Button></p>}
    <details className="mt-3">
      <summary className="cursor-pointer text-sm font-medium">六种账号类型策略</summary>
      {draft && <fieldset disabled={save.isPending} className="mt-3 space-y-3">
        <p className="text-xs text-muted-foreground">空白表示继承全局默认，0 表示不限。修改类型策略不会覆盖单账号设置。</p>
        <div className="overflow-x-auto"><table className="w-full text-left text-xs">
          <thead><tr className="border-b"><th className="py-2">类型</th><th>账号 / 单独配置</th><th className="min-w-28">每账号并发上限</th><th className="min-w-28">每账号 RPM 上限</th></tr></thead>
          <tbody>{TIERS.map(tier => <tr key={tier} className="border-b last:border-0">
            <td className="py-2 font-semibold">{TIER_NAMES[tier]}</td>
            <td>{rows.filter(r => r.effective.tier === tier).length} / {rows.filter(r => r.effective.tier === tier && (r.settings.concurrency !== null || r.settings.rpm !== null)).length}</td>
            {(['concurrency', 'rpm'] as const).map(field => <td key={field} className="p-1"><LimitInput label={`${TIER_NAMES[tier]} ${field === 'rpm' ? 'RPM' : '并发'}上限`}
              value={draft.tiers[tier]?.[field] ?? null} onChange={value => edit({ ...draft, tiers: { ...draft.tiers,
                [tier]: { concurrency: null, rpm: null, ...draft.tiers[tier], [field]: value } } })} /></td>)}
          </tr>)}</tbody>
        </table></div>
        <div className="flex items-center gap-3 text-xs"><label htmlFor="traffic-default-concurrency">全局默认并发</label>
          <Input id="traffic-default-concurrency" className="h-8 w-28" type="number" min={0} max={100000} value={draft.defaultConcurrency}
            onChange={e => edit({ ...draft, defaultConcurrency: Number(e.target.value) })} />
          <span className="text-muted-foreground">默认 RPM 沿用系统设置中的单账号限流值</span></div>
        <label className="flex items-center gap-3 text-sm"><Switch checked={draft.autoDisableQuota} onCheckedChange={value => edit({ ...draft, autoDisableQuota: value })} />额度达到上限时自动禁用</label>
        <label className="flex items-center gap-3 text-sm"><Switch checked={draft.disableAtBaseQuota} disabled={!draft.autoDisableQuota}
          onCheckedChange={value => edit({ ...draft, disableAtBaseQuota: value })} />已开启付费超额的账号，基础额度耗尽时也禁用</label>
        <p className="text-xs text-muted-foreground">关闭自动禁用不会恢复已禁用账号。开启后，以最新有效额度或上游明确的额度耗尽结果判断。</p>
        <Button size="sm" disabled={!dirty || save.isPending} onClick={() => {
          if (!valid(draft.defaultConcurrency) || Object.values(draft.tiers).some(l => l && (!valid(l.concurrency) || !valid(l.rpm)))) return toast.error('请输入 0～100000 的整数')
          const patch: Partial<Policies> = { tiers: {} }
          for (const tier of TIERS) {
            if (JSON.stringify(draft.tiers[tier]) !== JSON.stringify(original?.tiers[tier])) patch.tiers![tier] = draft.tiers[tier]
          }
          if (draft.defaultConcurrency !== original?.defaultConcurrency) patch.defaultConcurrency = draft.defaultConcurrency
          if (draft.autoDisableQuota !== original?.autoDisableQuota) patch.autoDisableQuota = draft.autoDisableQuota
          if (draft.disableAtBaseQuota !== original?.disableAtBaseQuota) patch.disableAtBaseQuota = draft.disableAtBaseQuota
          save.mutate(patch)
        }}>{save.isPending ? '保存中…' : '保存类型策略'}</Button>
      </fieldset>}
    </details>
  </section>
}

export function CredentialTrafficControls({ id, updatedAt, disabledReason }: { id: number; updatedAt?: number; disabledReason?: string }) {
  const client = useQueryClient()
  const { rows, stale, errors, updatedAt: sampledAt } = useContext(TrafficContext)
  const row = rows.find(r => r.id === id)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState<CredentialTraffic>({ tier: null, concurrency: null, rpm: null })
  const actionLock = useRef(false)
  const save = useMutation({ mutationFn: (action: { kind: 'save'; settings: CredentialTraffic } | { kind: 'restore' }) =>
    action.kind === 'restore' ? restoreCredentialTrafficLimits(id) : saveCredentialTraffic(id, action.settings), onSuccess: () => {
    client.invalidateQueries({ queryKey: ['credential-traffic'] }); client.invalidateQueries({ queryKey: ['credentials'] })
    setEditing(false); toast.success('账号策略已保存')
  }, onError: e => toast.error(extractErrorMessage(e)) })
  const submit = async (action: { kind: 'save'; settings: CredentialTraffic } | { kind: 'restore' }) => {
    if (actionLock.current) return
    actionLock.current = true
    try { await save.mutateAsync(action) }
    catch { /* The mutation displays the API error and leaves the editor open. */ }
    finally { actionLock.current = false }
  }
  if (!row) return <p className="text-xs text-muted-foreground">实时限流状态加载中…</p>
  return <div className="space-y-2 border-t pt-3 text-xs" data-no-rect-select onPointerDown={e => e.stopPropagation()}>
    <div className="flex flex-wrap justify-between gap-2"><span className="font-medium">{TIER_NAMES[row.effective.tier]} · 并发 {row.concurrency} / {cap(row.effective.concurrency)} · RPM {row.rpm} / {cap(row.effective.rpm)}</span>
      <button type="button" className="text-primary underline" disabled={save.isPending} onClick={() => {
        if (actionLock.current) return
        setDraft(row.settings); setEditing(!editing)
      }}>单账号设置</button></div>
    <p className="text-muted-foreground">并发：{origin(row.effective.concurrencySource)} · RPM：{origin(row.effective.rpmSource)}{row.limited ? ' · 已达限流上限' : ''}{stale ? ' · 实时数据已过期' : ` · ${new Date(sampledAt).toLocaleTimeString()}`}</p>
    <p className="text-muted-foreground">额度更新：{updatedAt ? new Date(updatedAt * 1000).toLocaleString() : '未查询'}{disabledReason === 'QuotaExceeded' ? ' · 因额度耗尽已禁用' : ''}</p>
    {errors[id] && <p className="text-amber-600">{errors[id]}</p>}
    {editing && <fieldset disabled={save.isPending} className="space-y-2 rounded-md bg-muted/40 p-3">
      <label className="flex items-center gap-2">策略类型<select aria-label={`凭据 ${id} 类型`} className="rounded border bg-background p-1" value={draft.tier ?? ''}
        onChange={e => setDraft({ ...draft, tier: (e.target.value || null) as AccountTier | null })}>
        <option value="">自动识别</option>{TIERS.map(t => <option key={t} value={t}>{TIER_NAMES[t]}</option>)}
      </select></label>
      <div className="grid grid-cols-2 gap-2"><label>并发上限<LimitInput label={`凭据 ${id} 并发上限`} value={draft.concurrency} onChange={value => setDraft({ ...draft, concurrency: value })} /></label>
        <label>RPM 上限<LimitInput label={`凭据 ${id} RPM 上限`} value={draft.rpm} onChange={value => setDraft({ ...draft, rpm: value })} /></label></div>
      <p className="text-muted-foreground">空白继承类型策略，0 表示不限。单独设置后不随类型策略变化。</p>
      <div className="flex flex-wrap gap-2"><Button size="sm" disabled={save.isPending} onClick={() => {
        if (!valid(draft.concurrency) || !valid(draft.rpm)) return toast.error('请输入 0～100000 的整数')
        void submit({ kind: 'save', settings: draft })
      }}>保存账号设置</Button><Button size="sm" variant="outline" disabled={save.isPending}
        onClick={() => submit({ kind: 'restore' })}>恢复继承类型策略</Button></div>
    </fieldset>}
  </div>
}
