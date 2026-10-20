import { api } from './credentials'

export const TIERS = ['free', 'pro', 'pro_plus', 'max', 'power', 'student'] as const
export type AccountTier = typeof TIERS[number] | 'unknown'
export const POLICY_TIERS: AccountTier[] = [...TIERS, 'unknown']
export const ACCOUNT_TYPES = ['enterprise', 'github', 'google', 'builder_id', 'iam_sso', 'entra_id', 'api_key', 'unknown'] as const
export type AccountType = typeof ACCOUNT_TYPES[number]
export const ACCOUNT_TYPE_NAMES: Record<AccountType, string> = {
  enterprise: 'Enterprise', github: 'GitHub', google: 'Google', builder_id: 'Builder ID',
  iam_sso: 'IAM SSO', entra_id: 'Entra ID', api_key: 'API Key', unknown: '未识别类型',
}
export const TIER_NAMES: Record<AccountTier, string> = {
  free: 'FREE', pro: 'PRO', pro_plus: 'PRO+', max: 'MAX', power: 'POWER', student: 'STUDENT', unknown: '待识别',
}
export interface Limits { concurrency: number | null; rpm: number | null }
export interface CredentialTraffic extends Limits { tier: AccountTier | null }
export interface Policies {
  tiers: Partial<Record<AccountTier, Limits>>
  accountTypes?: Partial<Record<AccountType, AccountTypePolicy>>
  defaultConcurrency: number
  autoDisableQuota: boolean
  disableAtBaseQuota: boolean
}
export interface AccountTypePolicy { defaults: Limits; tiers: Partial<Record<AccountTier, Limits>> }
export interface PolicyPatch extends Partial<Omit<Policies, 'tiers' | 'accountTypes'>> {
  tiers?: Partial<Record<AccountTier, Partial<Limits>>>
  accountTypes?: Partial<Record<AccountType, { defaults?: Partial<Limits>; tiers?: Partial<Record<AccountTier, Partial<Limits>>> }>>
}
export interface TrafficStatus {
  id: number
  settings: CredentialTraffic
  effective: { accountType?: AccountType; tier: AccountTier; concurrency: number; rpm: number; concurrencySource: string; rpmSource: string }
  concurrency: number
  rpm: number
  limited: boolean
}
export interface RefreshJob {
  running: boolean; total: number; completed: number; failed: number; skipped: number
  errors: Record<string, string>; finishedAt: string | null
}
export const getPolicies = async () => (await api.get<Policies>('/config/traffic-policies')).data
export const savePolicies = async (policy: PolicyPatch) => (await api.patch<Policies>('/config/traffic-policies', policy)).data
export const getTraffic = async () => (await api.get<TrafficStatus[]>('/credentials/traffic')).data
export const saveCredentialTraffic = async (id: number, settings: CredentialTraffic) =>
  (await api.put(`/credentials/${id}/traffic`, settings)).data
export const restoreCredentialTrafficLimits = async (id: number) =>
  (await api.post(`/credentials/${id}/traffic/restore-limits`)).data
export const startBalanceRefresh = async () => (await api.post<RefreshJob>('/credentials/refresh-balances')).data
export const getBalanceRefresh = async (signal?: AbortSignal) =>
  (await api.get<RefreshJob>('/credentials/refresh-balances', { signal })).data
