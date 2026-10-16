import { expect, test } from 'bun:test'
import { api, getCredentialBalance, refreshCredentialBalance } from '../src/api/credentials'

test('manual review posts to the fresh-balance endpoint while ordinary reads keep their cached GET', async () => {
  const adapter = api.defaults.adapter
  const requests: { method?: string; url?: string; timeout?: number }[] = []
  api.defaults.adapter = async config => {
    requests.push({ method: config.method, url: config.url, timeout: config.timeout })
    return { config, status: 200, statusText: 'OK', headers: {}, data: { id: 42, remaining: 80 } }
  }
  try {
    expect((await refreshCredentialBalance(42)).remaining).toBe(80)
    await getCredentialBalance(42)
    expect(requests).toEqual([
      { method: 'post', url: '/credentials/42/balance/refresh', timeout: 120000 },
      { method: 'get', url: '/credentials/42/balance', timeout: 15000 },
    ])
  } finally { api.defaults.adapter = adapter }
})
