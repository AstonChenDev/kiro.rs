import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { credentialTier, detectTier } from '../src/lib/subscription-tier'
import type { AccountTier, TrafficStatus } from '../src/api/traffic'

function traffic(tier: AccountTier = 'unknown', manual: AccountTier | null = null): TrafficStatus {
  return { id: 1, settings: { tier: manual, concurrency: null, rpm: null },
    effective: { tier, concurrency: 0, rpm: 0, concurrencySource: 'global', rpmSource: 'global' },
    concurrency: 0, rpm: 0, limited: false }
}

test('unknown traffic data does not classify a recognized MAX subscription as unknown', () => {
  expect(credentialTier('KIRO MAX', traffic())).toBe('max')
  expect(credentialTier('KIRO MAX')).toBe('max')
  expect(credentialTier('KIRO PRO MAX', traffic())).toBe('max')
})

test('all six recognized subscriptions survive unknown traffic snapshots', () => {
  for (const [title, tier] of [['FREE', 'free'], ['PRO', 'pro'], ['PRO+', 'pro_plus'],
    ['MAX', 'max'], ['POWER', 'power'], ['STUDENT', 'student']] as const) {
    expect(credentialTier(`KIRO ${title}`, traffic())).toBe(tier)
    expect(detectTier(`kiro ${title.toLowerCase()}`)).toBe(tier)
  }
})

test('manual strategy type still wins, including an explicit unknown', () => {
  expect(credentialTier('KIRO MAX', traffic('power', 'power'))).toBe('power')
  expect(credentialTier('KIRO MAX', traffic('unknown', 'unknown'))).toBe('unknown')
})

test('fresh recognized titles override stale automatic tiers; missing titles retain available backend data', () => {
  expect(credentialTier('KIRO MAX', traffic('pro'))).toBe('max')
  expect(credentialTier(null, traffic('max'))).toBe('max')
  expect(credentialTier(undefined)).toBe('unknown')
  expect(credentialTier('Unrecognized subscription', traffic())).toBe('unknown')
})

// Exercise the actual dashboard filter with independently arriving snapshots.
const source = readFileSync(new URL('../src/components/dashboard.tsx', import.meta.url), 'utf8')
const start = source.indexOf('    if (tierFilter.size > 0) {', source.indexOf('const filteredCredentials'))
const end = source.indexOf('    const q =', start)
if (start < 0 || end < 0) throw new Error('Dashboard subscription filter missing')
const filterCode = new Bun.Transpiler({ loader: 'tsx' }).transformSync(source.slice(start, end))
const filter = new Function('out', 'tierFilter', 'trafficRows', 'balanceMap', 'credentialTier',
  `${filterCode}; return out;`)
const credential = { id: 1, subscriptionTitle: 'KIRO PRO', balance: { subscriptionTitle: 'KIRO PRO' } }

test('dashboard uses freshly refreshed MAX balance and excludes it from unknown results', () => {
  const latest = new Map([[1, { subscriptionTitle: 'KIRO PRO MAX' }]])
  expect(filter([credential], new Set(['max']), [traffic()], latest, credentialTier)).toEqual([credential])
  expect(filter([credential], new Set(['unknown']), [traffic()], latest, credentialTier)).toEqual([])
})

test('dashboard falls back to cached balance then persisted subscription title', () => {
  const cached = { ...credential, balance: { subscriptionTitle: 'KIRO MAX' } }
  const persisted = { id: 1, subscriptionTitle: 'KIRO MAX' }
  for (const c of [cached, persisted]) {
    expect(filter([c], new Set(['max']), [traffic()], new Map(), credentialTier)).toEqual([c])
  }
})

test('subscription menu places MAX above POWER and retains every supported tier', () => {
  const options = source.match(/const TIER_OPTIONS[^=]*= (\[[\s\S]*?\]);/)?.[1]
  if (!options) throw new Error('Subscription menu missing')
  const values = new Function(`return ${options}`)().map((x: { value: string }) => x.value)
  expect(values).toEqual(['free', 'pro', 'pro_plus', 'max', 'power', 'student', 'unknown'])
})


test('MAX and STUDENT match complete backend subscription names', () => {
  for (const title of ['KIRO PRO MAX', ' kiro pro max ', 'PRO MAX', 'KIRO MAX', 'MAX']) {
    expect(detectTier(title)).toBe('max')
  }
  for (const title of ['KIRO STUDENT', ' kiro student ', 'STUDENT']) {
    expect(detectTier(title)).toBe('student')
  }
})

test('unrecognized MAX and STUDENT titles do not fall through to legacy PRO detection', () => {
  for (const title of ['MAX TRIAL', 'STUDENT PROMO', 'KIRO PRO MAXIMUM', 'KIRO STUDENT PRO', 'KIRO PRO MAX TRIAL']) {
    expect(detectTier(title)).toBe('unknown')
    expect(credentialTier(title, traffic())).toBe('unknown')
    expect(filter([{ id: 1, subscriptionTitle: title }], new Set(['max', 'student', 'pro']), [traffic()], new Map(), credentialTier)).toEqual([])
  }
})

test('canonical MAX and STUDENT titles filter correctly while manual settings still win', () => {
  for (const [title, tier] of [['KIRO PRO MAX', 'max'], ['KIRO STUDENT', 'student']] as const) {
    const c = { id: 1, subscriptionTitle: title }
    expect(filter([c], new Set([tier]), [traffic()], new Map(), credentialTier)).toEqual([c])
    expect(filter([c], new Set(['unknown']), [traffic()], new Map(), credentialTier)).toEqual([])
    expect(credentialTier(title, traffic('power', 'power'))).toBe('power')
  }
})
