import { deleteCredentialsWithVerification } from '../src/lib/credential-delete-result'
import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { AxiosError, isAxiosError } from 'axios'
import { createCredentialBatchState } from '../src/lib/credential-batch-state'

const source = readFileSync(new URL('../src/components/dashboard.tsx', import.meta.url), 'utf8')
const start = source.indexOf('  const handleBatchDelete = async () => {')
const end = source.indexOf('  const handleBatchResetFailure', start)
if (start < 0 || end < 0) throw new Error('Selected-delete handler missing')
const code = new Bun.Transpiler({ loader: 'tsx' }).transformSync(source.slice(start, end))
const httpError = (status: number) => new AxiosError(`HTTP ${status}`, undefined, undefined, undefined, { status } as never)
function gate() {
  let release!: () => void
  const promise = new Promise<void>(resolve => { release = resolve })
  return { promise, release }
}
const tick = async () => { for (let i = 0; i < 30; i++) await Promise.resolve() }
function fixture() {
  const store = createCredentialBatchState()
  const server = new Set([1, 42, 800])
  store.setSelectedIds(new Set(server))
  const writes: number[] = [], messages: string[] = [], invalidate: string[] = []
  let reads = 0
  const api = {
    remove: async (id: number) => { if (!server.delete(id)) throw httpError(404) },
    read: async () => [...server].map(id => ({ id })),
    confirm: async () => true,
  }
  function mount() {
    const args = {
      deleteCredentialsWithVerification,
      ...store, ...store.getSnapshot(), isAxiosError,
      confirm: () => api.confirm(),
      deleteCredentialRequest: async (id: number) => { writes.push(id); await api.remove(id) },
      getCredentials: async () => { reads++; return { credentials: await api.read() } },
      extractErrorMessage: String,
      queryClient: { invalidateQueries: async ({ queryKey }: { queryKey: string[] }) => { invalidate.push(queryKey[0]) } },
      toast: { success: (m: string) => messages.push(m), warning: (m: string) => messages.push(m), error: (m: string) => messages.push(m) },
    }
    return new Function(...Object.keys(args), `${code}; return handleBatchDelete;`)(...Object.values(args)) as () => Promise<void>
  }
  return { store, server, api, mount, run: () => mount()(), state: () => ({ selected: [...store.getSnapshot().selectedIds],
    busy: store.getSnapshot().batchDeleting, locked: store.batchDeleteLock.current,
    failures: store.getSnapshot().deleteErrors, invalidate, messages, writes, reads }) }
}
test('deletes all selected IDs across pages and refreshes IP ownership as well as credentials', async () => {
  const f = fixture(); await f.run()
  expect(f.state()).toMatchObject({ writes: [1, 42, 800], selected: [], busy: false, locked: false, failures: [], reads: 0,
    invalidate: ['credentials', 'credential-traffic', 'static-ips', 'proxy-pool', 'stats'] })
})
test('lost committed responses are settled by one fresh read, preserving new selections', async () => {
  const f = fixture()
  f.api.remove = async id => { f.server.delete(id); f.store.setSelectedIds(current => new Set([...current, 999])); throw Error('lost response') }
  await f.run()
  expect(f.state()).toMatchObject({ selected: [999], failures: [], reads: 1, locked: false })
  expect(f.state().messages.at(-1)).toContain('已完成删除 3')
})
test('a retry returning 404 removes already absent IDs only after a fresh successful read', async () => {
  const f = fixture(); f.server.clear(); await f.run()
  expect(f.state()).toMatchObject({ selected: [], failures: [], reads: 1 })
})
test('uncommitted network failures stay selected with their original reason', async () => {
  const f = fixture()
  f.api.remove = async id => { if (id === 42) throw Error('offline'); f.server.delete(id) }
  await f.run()
  expect(f.state()).toMatchObject({ writes: [1, 42, 800], selected: [42], busy: false, locked: false, reads: 1 })
  expect(f.state().failures[0].message).toContain('仍存在')
  expect(f.state().failures[0].message).toContain('offline')
})
test('failed verification leaves committed deletions unconfirmed until a later retry can read state', async () => {
  const f = fixture()
  f.api.remove = async id => { f.server.delete(id); throw Error('response lost') }
  f.api.read = async () => { throw Error('offline') }
  await f.run()
  expect(f.state()).toMatchObject({ selected: [1, 42, 800], locked: false, busy: false })
  expect(f.state().failures.every(item => item.message.includes('结果未确认'))).toBe(true)
  expect(f.state().messages.at(-1)).toContain('3 个结果未确认')
  f.api.remove = async () => { throw httpError(404) }
  f.api.read = async () => []
  await f.run()
  expect(f.state()).toMatchObject({ selected: [], failures: [], reads: 2, locked: false })
})
test('404 without a successful fresh verification is not reported as deletion success', async () => {
  const f = fixture(); f.server.clear(); f.api.read = async () => { throw Error('offline') }
  await f.run()
  expect(f.state().selected).toEqual([1, 42, 800]); expect(f.state().failures).toHaveLength(3)
  expect(f.state().messages.at(-1)).toContain('已完成删除 0')
})
test('server errors can be reconciled but explicit permission rejections retain their reason', async () => {
  const f = fixture()
  f.api.remove = async id => { if (id === 42) throw httpError(403); f.server.delete(id); throw httpError(502) }
  await f.run()
  expect(f.state()).toMatchObject({ selected: [42], reads: 1, failures: [{ id: 42, message: 'AxiosError: HTTP 403' }] })
})
test('a 404 with the account still present is not assumed to mean deletion succeeded', async () => {
  const f = fixture(); f.api.remove = async () => { throw httpError(404) }; await f.run()
  expect(f.state().selected).toEqual([1, 42, 800]); expect(f.state().failures).toHaveLength(3)
})
test('cancellation leaves selections untouched and sends no deletes or reads', async () => {
  const f = fixture(); f.api.confirm = async () => false; await f.run()
  expect(f.state()).toMatchObject({ writes: [], reads: 0, selected: [1, 42, 800], busy: false, locked: false })
})
test('duplicate clicks cannot issue duplicate deletes during writes or verification, including after remount', async () => {
  const f = fixture(), hold = gate()
  f.api.remove = async id => { f.server.delete(id); throw Error('lost') }
  f.api.read = async () => { await hold.promise; return [] }
  const first = f.run(); await tick(); await f.mount()()
  expect(f.state().writes).toEqual([1, 42, 800]); expect(f.state().locked).toBe(true)
  let singleCalls = 0; await f.store.runSingleCredentialChange(async () => { singleCalls++ })
  expect(singleCalls).toBe(0)
  hold.release(); await first
  expect(f.state()).toMatchObject({ selected: [], reads: 1, failures: [], busy: false, locked: false })
})
