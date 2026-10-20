import { ACCOUNT_TYPES, POLICY_TIERS, type Limits, type Policies, type PolicyPatch } from '@/api/traffic'

const valid = (v: number | null) => v === null || (Number.isInteger(v) && v >= 0 && v <= 100000)
const changedLimits = (before?: Partial<Limits>, after?: Partial<Limits>): Partial<Limits> => {
  const patch: Partial<Limits> = {}
  for (const field of ['concurrency', 'rpm'] as const) {
    const value = after?.[field] ?? null
    if (value !== (before?.[field] ?? null)) patch[field] = value
  }
  return patch
}

/** Send only edited leaves: concurrent editors must not overwrite other types/tiers. */
export function buildPolicyPatch(original: Policies | null, draft: Policies): PolicyPatch {
  const limits = [...Object.values(draft.tiers), ...Object.values(draft.accountTypes ?? {}).flatMap(p => p ? [p.defaults, ...Object.values(p.tiers)] : [])]
  if (!valid(draft.defaultConcurrency) || limits.some(l => l && (!valid(l.concurrency) || !valid(l.rpm)))) {
    throw new Error('请输入 0～100000 的整数')
  }
  const patch: PolicyPatch = { tiers: {} }
  for (const tier of POLICY_TIERS) {
    const delta = changedLimits(original?.tiers[tier], draft.tiers[tier])
    if (Object.keys(delta).length) patch.tiers![tier] = delta
  }
  for (const type of ACCOUNT_TYPES) {
    const before = original?.accountTypes?.[type], after = draft.accountTypes?.[type]
    const delta: NonNullable<PolicyPatch['accountTypes']>[typeof type] = {}
    const defaults = changedLimits(before?.defaults, after?.defaults)
    if (Object.keys(defaults).length) delta.defaults = defaults
    for (const tier of POLICY_TIERS) {
      const fields = changedLimits(before?.tiers[tier], after?.tiers[tier])
      if (Object.keys(fields).length) (delta.tiers ??= {})[tier] = fields
    }
    if (Object.keys(delta).length) (patch.accountTypes ??= {})[type] = delta
  }
  if (draft.defaultConcurrency !== original?.defaultConcurrency) patch.defaultConcurrency = draft.defaultConcurrency
  if (draft.autoDisableQuota !== original?.autoDisableQuota) patch.autoDisableQuota = draft.autoDisableQuota
  if (draft.disableAtBaseQuota !== original?.disableAtBaseQuota) patch.disableAtBaseQuota = draft.disableAtBaseQuota
  return patch
}

export function trafficPolicySource(source: string): string {
  return ({credential:'单账号', account_type_tier:'登录类型＋订阅等级', account_type:'登录类型默认', tier:'通用订阅等级', global:'全局默认'} as Record<string,string>)[source] ?? '全局默认'
}
