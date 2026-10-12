import { describe, expect, test } from 'bun:test'
import { normalizeKamAccount } from '../src/lib/kam-import'

describe('credential import traffic policies', () => {
  for (const traffic of [
    { tier: 'power', concurrency: 150, rpm: 100 },
    { tier: 'student', concurrency: 0, rpm: null },
    { concurrency: null, rpm: 0 },
  ]) {
    test(`preserves overrides in flat and nested formats: ${JSON.stringify(traffic)}`, () => {
      for (const account of [
        { refreshToken: 'fixture', traffic },
        { credentials: { refreshToken: 'fixture' }, traffic },
      ]) {
        const normalized = normalizeKamAccount(account) as { traffic: unknown; credentials: { refreshToken: string } }
        expect(normalized.traffic).toEqual(traffic)
        expect(normalized.credentials.refreshToken).toBe('fixture')
      }
    })
  }
  test('legacy imports without traffic keep inheritance', () => {
    const normalized = normalizeKamAccount({ refreshToken: 'fixture' }) as { traffic?: unknown }
    expect(normalized.traffic).toBeUndefined()
  })
})
