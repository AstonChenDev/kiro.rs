import { deleteCredentialsWithVerification } from '../src/lib/credential-delete-result'
import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { createCredentialBatchState } from '../src/lib/credential-batch-state'

const dashboard = readFileSync(new URL('../src/components/dashboard.tsx', import.meta.url), 'utf8')
const card = readFileSync(new URL('../src/components/credential-card.tsx', import.meta.url), 'utf8')
const extract = (source: string, start: string, end: string) => source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)))
const compile = (source: string) => new Bun.Transpiler({ loader: 'tsx' }).transformSync(source)
const batchCode = compile(extract(dashboard, '  const handleBatchEnable =', '  const handleBatchForceRefresh ='))
const cardCode = compile(
  extract(card, '  const handleToggleDisabled =', '  const handlePriorityChange =') +
  extract(card, '  const handleReset =', '  const handleForceRefresh =') +
  extract(card, '  const handleDelete =', '  const authLabel ='),
)
const tick = async () => { for (let i = 0; i < 30; i++) await Promise.resolve() }
function gate() {
  let release!: () => void
  const promise = new Promise<void>(r => { release = r })
  return { promise, release }
}
function fixture(count = 1) {
  const store = createCredentialBatchState()
  const server = Array.from({ length: count }, (_, i) => ({ id: i + 1, disabled: true, failureCount: 1 }))
  const writes: { id: number; disabled: boolean; source: string }[] = [], balances: number[] = [], errors: string[] = []
  let restores = 0, deletes = 0
  const api = { singleWrite: async () => {}, batchWrite: async () => {}, balance: async () => {} }
  store.setSelectedIds(new Set(server.map(c => c.id)))
  const write = (id: number, disabled: boolean, source: string) => {
    writes.push({ id, disabled, source }); server.find(c => c.id === id)!.disabled = disabled
  }
  const toast = { success: () => {}, warning: () => {}, error: (s: string) => errors.push(s) }
  function batch() {
    const args = {
      deleteCredentialsWithVerification,
      ...store, ...store.getSnapshot(),
      getCredentials: async () => ({ credentials: structuredClone(server) }),
      setCredentialDisabled: async (id: number, disabled: boolean) => { await api.batchWrite(); write(id, disabled, 'batch') },
      getCredentialBalance: async (id: number) => { balances.push(id); await api.balance(); return { id } },
      getStaticIps: async () => ({ waitingCredentialIds: [] }), setBalanceMap: () => {},
      isAxiosError: () => false, extractErrorMessage: String,
      queryClient: { invalidateQueries: async () => {} }, toast,
      data: { credentials: structuredClone(server) },
      resetFailure: async () => { restores++ }, deselectAll: () => store.setSelectedIds(new Set()),
      confirm: async () => true, deleteCredentialRequest: async () => { deletes++ },
    }
    return new Function(...Object.keys(args), `${batchCode};return {enable:handleBatchEnable,disable:handleBatchDisable,restore:handleBatchResetFailure,remove:handleBatchDelete}`)(...Object.values(args)) as Record<'enable' | 'disable' | 'restore' | 'remove', () => Promise<void>>
  }
  function single(id = 1) {
    const args = {
      deleteCredentialsWithVerification,
      runSingleCredentialChange: store.runSingleCredentialChange,
      credential: { ...server.find(c => c.id === id)! },
      setDisabled: { mutateAsync: async (v: { id: number; disabled: boolean }) => {
        await api.singleWrite(); write(v.id, v.disabled, 'single'); return { message: 'saved' }
      } },
      resetFailure: { mutateAsync: async () => { await api.singleWrite(); restores++; return { message: 'saved' } } },
      deleteCredential: { mutateAsync: async () => { await api.singleWrite(); deletes++; return { message: 'saved' } } },
      onRefreshBalance: async () => { balances.push(id); await api.balance() },
      setShowDeleteDialog: () => {}, toast, extractErrorMessage: String,
    }
    return new Function(...Object.keys(args), `${cardCode};return {toggle:handleToggleDisabled,restore:handleReset,remove:handleDelete}`)(...Object.values(args)) as Record<'toggle' | 'restore' | 'remove', () => Promise<void>>
  }
  return { store, server, writes, balances, errors, api, batch, single, counts: () => ({ restores, deletes }) }
}

test('pending card enable blocks bulk disable; a later explicit disable determines final state', async () => {
  const f = fixture(), hold = gate(); f.api.singleWrite = () => hold.promise
  const oldCard = f.single(), pending = oldCard.toggle()
  expect(f.store.getSnapshot().singleChanging).toBe(true)
  await f.batch().disable()
  expect(f.writes).toEqual([])
  hold.release(); await pending
  expect(f.server[0].disabled).toBe(false)
  await f.batch().disable()
  expect(f.server[0].disabled).toBe(true)
  expect(f.writes.map(w => w.source)).toEqual(['single', 'batch'])
})

test('bulk enable blocks card changes to queued accounts until the batch finishes', async () => {
  const f = fixture(4), hold = gate(); f.api.batchWrite = () => hold.promise
  const pending = f.batch().enable(); await tick()
  expect(f.writes).toHaveLength(0)
  await f.single(4).toggle(); await f.single(4).restore(); await f.single(4).remove()
  expect(f.writes).toHaveLength(0); expect(f.counts()).toEqual({ restores: 0, deletes: 0 })
  hold.release(); await pending
  expect(f.server[3].disabled).toBe(false)
  await f.single(4).toggle()
  expect(f.server[3].disabled).toBe(true)
})

for (const action of ['toggle', 'restore', 'remove'] as const) test(`card ${action} holds the app lock across remount and blocks all bulk writes`, async () => {
  const f = fixture(), hold = gate(); f.api.singleWrite = () => hold.promise
  const pending = f.single()[action]()
  const remountedBatch = f.batch()
  await remountedBatch.enable(); await remountedBatch.disable(); await remountedBatch.restore(); await remountedBatch.remove()
  await f.single()[action]() // A fresh card instance cannot repeat the pending operation.
  expect(f.writes).toEqual([]); expect(f.counts()).toEqual({ restores: 0, deletes: 0 })
  hold.release(); await pending
  expect(f.store.batchDisableLock.current).toBe(false)
  expect(f.store.getSnapshot().singleChanging).toBe(false)
  expect(f.writes.length + f.counts().restores + f.counts().deletes).toBe(1)
})

for (const lock of ['batchDisableLock', 'batchDeleteLock', 'batchRestoreLock'] as const) test(`${lock} synchronously blocks card writes even before React renders`, async () => {
  const f = fixture(), staleCard = f.single()
  f.store[lock].current = true
  await staleCard.toggle(); await staleCard.restore(); await staleCard.remove()
  expect(f.writes).toEqual([]); expect(f.counts()).toEqual({ restores: 0, deletes: 0 })
  expect(f.store[lock].current).toBe(true)
})

test('card quota check retains the lock until its auto-disable finishes', async () => {
  const f = fixture(), hold = gate()
  f.api.balance = async () => { await hold.promise; f.server[0].disabled = true }
  const pending = f.single().toggle(); await tick()
  expect(f.server[0].disabled).toBe(false)
  await f.batch().enable(); await f.single().toggle()
  expect(f.writes).toHaveLength(1)
  hold.release(); await pending
  expect(f.server[0].disabled).toBe(true)
  expect(f.store.getSnapshot().singleChanging).toBe(false)
})

for (const action of ['toggle', 'restore', 'remove'] as const) test(`failed card ${action} releases the lock and allows retry`, async () => {
  const f = fixture(); f.api.singleWrite = async () => { throw Error('offline') }
  await f.single()[action]()
  expect(f.errors[0]).toContain('offline')
  expect(f.store.batchDisableLock.current).toBe(false)
  expect(f.store.getSnapshot().singleChanging).toBe(false)
  f.api.singleWrite = async () => {}; await f.single()[action]()
  expect(f.writes.length + f.counts().restores + f.counts().deletes).toBe(1)
})

test('a rejected quota refresh cannot leave the status lock stuck', async () => {
  const f = fixture(); f.api.balance = async () => { throw Error('quota offline') }
  await f.single().toggle()
  expect(f.errors[0]).toContain('quota offline')
  expect(f.store.batchDisableLock.current).toBe(false)
  await f.batch().disable(); expect(f.server[0].disabled).toBe(true)
})
