import { deleteCredentialsWithVerification } from '../src/lib/credential-delete-result'
import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { MutationObserver, QueryClient } from '@tanstack/react-query'
import { createCredentialBatchState } from '../src/lib/credential-batch-state'

const source = readFileSync(new URL('../src/components/dashboard.tsx', import.meta.url), 'utf8')
const code = new Bun.Transpiler({ loader: 'tsx' }).transformSync(source.slice(
  source.indexOf('  const handleBatchQuotaReview ='), source.indexOf('  const handleBatchForceRefresh ='),
))
const tick = async () => { for (let i = 0; i < 30; i++) await Promise.resolve() }
function gate() {
  let release!: () => void
  const promise = new Promise<void>(resolve => { release = resolve })
  return { promise, release }
}
function fixture(count = 4) {
  const store = createCredentialBatchState()
  const server = Array.from({ length: count }, (_, i) => ({ id: i + 1, disabled: true, failureCount: 1, disabledReason: '' }))
  const writes: { id: number; disabled: boolean }[] = [], balances: number[] = []
  const api = {
    balance: async (id: number) => ({ id }),
    write: async () => {},
    ips: async () => ({ waitingCredentialIds: [] as number[] }),
    restore: async (id: number) => { server.find(c => c.id === id)!.disabled = false },
  }
  const selectAll = () => store.setSelectedIds(new Set(server.map(c => c.id)))
  selectAll()
  // Each call represents a new Dashboard mount. Only the app-owned store survives.
  function mount() {
    let snapshot = store.getSnapshot()
    const unmount = store.subscribe(() => { snapshot = store.getSnapshot() })
    const args = {
      deleteCredentialsWithVerification,
      ...store, ...snapshot,
      getCredentials: async () => ({ credentials: structuredClone(server) }),
      setCredentialDisabled: async (id: number, disabled: boolean) => { await api.write(); writes.push({ id, disabled }); server.find(c => c.id === id)!.disabled = disabled },
      getCredentialBalance: async () => { throw Error('quota review must bypass the cached GET') },
      refreshCredentialBalance: async (id: number) => { balances.push(id); return api.balance(id) },
      getStaticIps: () => api.ips(), setBalanceMap: () => {},
      isAxiosError: () => false, extractErrorMessage: String,
      queryClient: { invalidateQueries: async () => {} },
      toast: { success: () => {}, warning: () => {}, error: () => {} },
      data: { credentials: structuredClone(server) },
      resetFailure: (id: number) => api.restore(id),
      deselectAll: () => store.setSelectedIds(new Set()),
      confirm: async () => true, deleteCredentialRequest: async () => {},
    }
    const handlers = new Function(...Object.keys(args), `${code}; return { review: handleBatchQuotaReview, enable: handleBatchEnable, disable: handleBatchDisable, restore: handleBatchResetFailure, remove: handleBatchDelete }`)(...Object.values(args)) as Record<'review' | 'enable' | 'disable' | 'restore' | 'remove', () => Promise<void>>
    return { ...handlers, unmount, snapshot: () => snapshot }
  }
  return { store, server, writes, balances, api, mount, selectAll }
}

test('remount preserves enable state and prevents conflicting writes until completion', async () => {
  const f = fixture(), hold = gate(); f.api.write = () => hold.promise
  const old = f.mount(), running = old.enable(); await tick(); old.unmount()
  const fresh = f.mount(); expect(fresh.snapshot().batchEnabling).toBe(true)
  await fresh.disable(); await fresh.restore(); await fresh.remove(); await fresh.review()
  expect(f.writes).toHaveLength(0); expect(f.balances).toEqual([])
  hold.release(); await running
  expect(fresh.snapshot().batchEnabling).toBe(false)
  expect(fresh.snapshot().enableProgress.current).toBe(4)
  expect(f.store.batchDisableLock.current).toBe(false)
  f.selectAll(); await f.mount().disable()
  expect(f.server.every(c => c.disabled)).toBe(true); expect(f.writes).toHaveLength(8)
})

test('quota review queries selected disabled and enabled accounts without enabling any', async () => {
  const f = fixture(2); f.server[1].disabled = false
  await f.mount().review()
  expect(f.balances).toEqual([1, 2]); expect(f.writes).toEqual([])
  expect(f.server[0].disabled).toBe(true); expect(f.store.getSnapshot().selectedIds.size).toBe(2)
  expect(f.store.getSnapshot().quotaSummary).toContain('成功 2 个，失败 0 个')
})

test('quota review limits concurrency, survives remount, and excludes duplicate or conflicting actions', async () => {
  const f = fixture(), hold = gate(); f.api.balance = async id => { await hold.promise; return { id } }
  const old = f.mount(), running = old.review(); await tick(); old.unmount()
  expect(f.balances).toHaveLength(3)
  const fresh = f.mount(); expect(fresh.snapshot().batchQuotaReviewing).toBe(true)
  await fresh.review(); await fresh.enable(); await fresh.disable(); await fresh.restore(); await fresh.remove()
  expect(f.writes).toEqual([]); expect(f.balances).toHaveLength(3)
  let singleCalls = 0; await f.store.runSingleCredentialChange(async () => { singleCalls++ }); expect(singleCalls).toBe(0)
  hold.release(); await running
  expect(f.balances).toHaveLength(4); expect(fresh.snapshot().quotaProgress).toEqual({ current: 4, total: 4 })
  expect(fresh.snapshot().batchQuotaReviewing).toBe(false); expect(f.store.batchDisableLock.current).toBe(false)
})

test('quota failures complete off-page and retry only performs queries, never enable writes', async () => {
  const f = fixture(2), hold = gate()
  f.api.balance = async id => { await hold.promise; if (id === 1) throw Error('waiting for static IP'); return { id } }
  const old = f.mount(), running = old.review(); await tick(); old.unmount(); hold.release(); await running
  expect(f.store.getSnapshot().quotaErrors).toEqual([{ id: 1, message: 'Error: waiting for static IP' }])
  expect(f.store.getSnapshot().quotaSummary).toContain('成功 1 个，失败 1 个')
  f.store.setSelectedIds(new Set([1])); f.api.balance = async id => ({ id })
  await f.mount().review()
  expect(f.balances).toEqual([1, 2, 1]); expect(f.writes).toEqual([]); expect(f.store.getSnapshot().quotaErrors).toEqual([])
})

test('quota review preserves server auto-disable behavior and does not re-enable exhausted accounts', async () => {
  const f = fixture(1); f.server[0].disabled = false
  f.api.balance = async id => { Object.assign(f.server[0], { disabled: true, disabledReason: 'QuotaExceeded' }); return { id } }
  await f.mount().review()
  expect(f.server[0].disabled).toBe(true); expect(f.writes).toEqual([])
})

for (const reject of [false, true]) test(`restore releases the shared lock after observer unmount (reject=${reject})`, async () => {
  const f = fixture(1), hold = gate(), client = new QueryClient()
  const observer = new MutationObserver(client, { mutationFn: async (_id: number) => {
    await hold.promise
    if (reject) throw Error('failed')
    f.server[0].disabled = false
  } })
  const unsubscribe = observer.subscribe(() => {})
  f.api.restore = id => observer.mutate(id)
  const old = f.mount(), running = old.restore(); await tick(); old.unmount(); unsubscribe()
  const fresh = f.mount(); await fresh.enable(); await fresh.remove(); await fresh.review()
  expect(f.writes).toHaveLength(0); expect(fresh.snapshot().batchRestoring).toBe(true)
  hold.release(); await running
  expect(f.store.batchRestoreLock.current).toBe(false); expect(fresh.snapshot().batchRestoring).toBe(false)
  client.clear()
})

test('logged-in app owns shared state outside routes; independent actions have independent progress', () => {
  const app = readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8')
  expect(app).toMatch(/<CredentialBatchProvider>\s*<AppMain[^>]+\/>\s*<\/CredentialBatchProvider>/)
  expect(source).toContain('} = useCredentialBatchState();')
  expect(source.match(/onClick={handleBatchQuotaReview}/g)).toHaveLength(2)
  const first = createCredentialBatchState(); first.batchDisableLock.current = true; first.setSelectedIds(new Set([1]))
  const next = createCredentialBatchState()
  expect(next.batchDisableLock.current).toBe(false); expect(next.getSnapshot().selectedIds.size).toBe(0)
  expect(next.getSnapshot().batchQuotaReviewing).toBe(false)
})
