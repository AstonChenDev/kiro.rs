import type { TrafficStatus } from '@/api/traffic'

export type Tier = 'free' | 'pro' | 'pro_plus' | 'max' | 'power' | 'student' | 'unknown'

/** 根据订阅标题推断分级（供列表筛选复用） */
export function detectTier(title?: string | null): Tier {
  if (!title) return 'unknown'
  const upper = title.toUpperCase()
  // Keep the added tiers aligned with AccountTier::detect on the server.
  const normalized = upper.trim().replace(/^KIRO /, '').trim()
  if (normalized === 'STUDENT') return 'student'
  if (normalized === 'MAX' || normalized === 'PRO MAX') return 'max'
  // Unknown variants must not fall through to the legacy PRO substring rule.
  if (upper.includes('MAX') || upper.includes('STUDENT')) return 'unknown'
  if (upper.includes('POWER')) return 'power'
  if (upper.includes('PRO+') || upper.includes('PRO PLUS')) return 'pro_plus'
  if (upper.includes('PRO')) return 'pro'
  if (upper.includes('FREE')) return 'free'
  return 'unknown'
}

/** Manual strategy types win; an unknown/stale traffic snapshot must not hide a known subscription. */
export function credentialTier(title: string | null | undefined, traffic?: TrafficStatus): Tier {
  if (traffic?.settings.tier != null) return traffic.settings.tier
  const detected = detectTier(title)
  return detected !== 'unknown' ? detected : traffic?.effective.tier ?? 'unknown'
}
