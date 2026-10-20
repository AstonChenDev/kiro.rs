import { expect, test } from 'bun:test'
import { countByState, getDisposition, matchesStateFilter } from '../src/components/console/credential-state'
import type { CredentialStatusItem } from '../src/types/api'

function credential(disabled: boolean, disabledReason?: string): CredentialStatusItem {
  return { id: 1, disabled, disabledReason, isCurrent: false, refreshFailureCount: 0 } as CredentialStatusItem
}

test('waiting credentials have their own disabled category and no enable action', () => {
  const waiting = credential(true, 'WaitingStaticIp')
  expect(getDisposition(waiting)).toMatchObject({ state: 'waitingStaticIp', action: 'none', tone: 'warn' })
  expect(matchesStateFilter(waiting, null, 'waiting')).toBe(true)
  for (const filter of ['healthy', 'quota', 'throttled', 'dead'] as const) {
    expect(matchesStateFilter(waiting, null, filter)).toBe(false)
  }
  expect(countByState([waiting, credential(true, 'Manual'), credential(true, 'QuotaExceeded'), credential(false)], () => null))
    .toMatchObject({ waiting: 1, dead: 1, quota: 1, healthy: 1, total: 4 })
})

test('an account activated after IP binding leaves the waiting category', () => {
  const active = credential(false)
  expect(getDisposition(active).state).toBe('healthy')
  expect(matchesStateFilter(active, null, 'waiting')).toBe(false)
  expect(matchesStateFilter(active, null, 'healthy')).toBe(true)
})
