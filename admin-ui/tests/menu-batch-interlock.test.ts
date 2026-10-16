import { deleteCredentialsWithVerification } from '../src/lib/credential-delete-result'
import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { createCredentialBatchState } from '../src/lib/credential-batch-state'

const source = readFileSync(new URL('../src/components/dashboard.tsx', import.meta.url), 'utf8')
const extract = (start: string, end: string) => source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)))
const code = new Bun.Transpiler({ loader: 'tsx' }).transformSync(
  extract('  const handleBatchEnable =', '  const handleBatchForceRefresh =') +
  extract('  const handleClearAll =', '  const handleQueryCurrentPageInfo =') +
  extract('  const handleDisableQuotaExceeded =', '  // 一键开启超额'),
)
function gate<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>(r => { resolve = r })
  return { promise, resolve }
}
const tick = async () => { for (let i = 0; i < 30; i++) await Promise.resolve() }
function fixture() {
  const store = createCredentialBatchState()
  const server = [{ id: 1, disabled: true, failureCount: 1 }, { id: 2, disabled: true, failureCount: 1 }, { id: 3, disabled: false, failureCount: 1 }]
  const deleted: number[] = [], statusWrites: number[] = [], invalidated: string[] = [], messages: string[] = []
  let confirmations = 0, reads = 0, quotaCalls = 0, restores = 0
  store.setSelectedIds(new Set([1, 2]))
  const api = {
    confirm: async () => true,
    read: async () => structuredClone(server),
    remove: async (id: number) => { server.splice(server.findIndex(c => c.id === id), 1) },
    quota: async () => { server[2].disabled = true; return { disabledIds: [3], skippedIds: [] } },
  }
  function mount() {
    const args = {
      deleteCredentialsWithVerification,
      ...store, ...store.getSnapshot(), data: { credentials: structuredClone(server) }, quotaExceededCount: 1,
      confirm: async () => { confirmations++; return api.confirm() },
      getCredentials: async () => { reads++; return { credentials: await api.read() } },
      deleteCredentialRequest: async (id: number) => { deleted.push(id); await api.remove(id) },
      disableQuotaExceeded: async () => { quotaCalls++; return api.quota() },
      setCredentialDisabled: async (id: number, disabled: boolean) => { statusWrites.push(id); server.find(c => c.id === id)!.disabled = disabled },
      getCredentialBalance: async () => ({}), setBalanceMap: () => {}, getStaticIps: async () => ({ waitingCredentialIds: [] }),
      resetFailure: async () => { restores++ }, deselectAll: () => store.setSelectedIds(new Set()),
      extractErrorMessage: String, isAxiosError: () => false,
      queryClient: { invalidateQueries: async ({ queryKey }: { queryKey: string[] }) => { invalidated.push(queryKey[0]) } },
      toast: { success: (s: string) => messages.push(s), warning: (s: string) => messages.push(s), error: (s: string) => messages.push(s), info: (s: string) => messages.push(s) },
    }
    return new Function(...Object.keys(args), `${code};return {clear:handleClearAll,quota:handleDisableQuotaExceeded,enable:handleBatchEnable,disable:handleBatchDisable,remove:handleBatchDelete,restore:handleBatchResetFailure}`)(...Object.values(args)) as Record<'clear' | 'quota' | 'enable' | 'disable' | 'remove' | 'restore', () => Promise<void>>
  }
  return { store, server, deleted, statusWrites, invalidated, messages, api, mount, counts: () => ({ confirmations, reads, quotaCalls, restores }) }
}

for (const menu of ['clear', 'quota'] as const) {
  for (const lock of ['batchDisableLock', 'batchDeleteLock', 'batchRestoreLock'] as const) test(`${menu} cannot bypass ${lock}, even with a stale handler`, async () => {
    const f = fixture(), stale = f.mount(); f.store[lock].current = true
    await stale[menu]()
    expect(f.counts()).toEqual({ confirmations: 0, reads: 0, quotaCalls: 0, restores: 0 })
    expect(f.deleted).toEqual([]); expect(f.store[lock].current).toBe(true)
  })
  test(`${menu} holds its lock through confirmation across remount and rejects conflicting actions`, async () => {
    const f = fixture(), confirm = gate<boolean>(); f.api.confirm = () => confirm.promise
    const pending = f.mount()[menu](); await tick()
    const remounted = f.mount()
    await remounted.enable(); await remounted.disable(); await remounted.remove(); await remounted.restore()
    await remounted.clear(); await remounted.quota()
    let singleCalls = 0
    await f.store.runSingleCredentialChange(async () => { singleCalls++ })
    expect(singleCalls).toBe(0)
    expect(f.counts().confirmations).toBe(1); expect(f.statusWrites).toEqual([]); expect(f.deleted).toEqual([])
    confirm.resolve(false); await pending
    expect(f.store.batchDeleteLock.current).toBe(false); expect(f.store.batchDisableLock.current).toBe(false)
    expect(f.store.getSnapshot().batchDeleting).toBe(false); expect(f.store.getSnapshot().batchDisabling).toBe(false)
    expect(f.counts().quotaCalls).toBe(0)
    f.api.confirm = async () => true; await f.mount()[menu]()
    expect(menu === 'clear' ? f.deleted.length : f.counts().quotaCalls).toBe(menu === 'clear' ? 2 : 1)
  })
  test(`${menu} releases its lock if the confirmation dialog rejects`, async () => {
    const f = fixture(); f.api.confirm = async () => { throw Error('dialog closed') }
    await expect(f.mount()[menu]()).rejects.toThrow('dialog closed')
    expect(f.store.batchDeleteLock.current).toBe(false); expect(f.store.batchDisableLock.current).toBe(false)
  })
}

test('clear rechecks after confirmation, skips enabled accounts and never adds newly disabled targets', async () => {
  const f = fixture(), confirm = gate<boolean>(); f.api.confirm = () => confirm.promise
  const pending = f.mount().clear()
  f.server[0].disabled = false; f.server[2].disabled = true
  f.store.setSelectedIds(new Set([1, 2, 3, 99]))
  confirm.resolve(true); await pending
  expect(f.deleted).toEqual([2]); expect(f.server.map(c => c.id)).toEqual([1, 3])
  expect([...f.store.getSnapshot().selectedIds]).toEqual([1, 3, 99])
  expect(f.messages.at(-1)).toContain('跳过 1 个')
  expect(f.invalidated).toContain('static-ips'); expect(f.invalidated).toContain('proxy-pool')
})

test('clear skips accounts removed during confirmation without deleting a newly imported account', async () => {
  const f = fixture(), confirm = gate<boolean>(); f.api.confirm = () => confirm.promise
  const pending = f.mount().clear()
  f.server.splice(0, 1); f.server.push({ id: 4, disabled: true, failureCount: 0 })
  confirm.resolve(true); await pending
  expect(f.deleted).toEqual([2]); expect(f.server.some(c => c.id === 4)).toBe(true)
})

test('clear aborts on fresh read failure and permits retry without changing selection', async () => {
  const f = fixture(); f.api.read = async () => { throw Error('offline') }
  await f.mount().clear()
  expect(f.deleted).toEqual([]); expect([...f.store.getSnapshot().selectedIds]).toEqual([1, 2])
  expect(f.store.batchDeleteLock.current).toBe(false)
  expect(f.messages[0]).toContain('未执行清除')
  f.api.read = async () => structuredClone(f.server); await f.mount().clear()
  expect(f.deleted).toEqual([1, 2])
})

test('clear preserves failures and new selections, finishes off-page, then unlocks', async () => {
  const f = fixture(), hold = gate<void>()
  f.api.remove = async id => { await hold.promise; if (id === 1) throw Error('delete failed'); f.server.splice(f.server.findIndex(c => c.id === id), 1) }
  const pending = f.mount().clear(); await tick()
  f.store.setSelectedIds(new Set([99]))
  await f.mount().enable(); expect(f.statusWrites).toEqual([])
  hold.resolve(); await pending
  expect([...f.store.getSnapshot().selectedIds].sort()).toEqual([1, 99])
  expect(f.store.getSnapshot().deleteErrors).toEqual([{ id: 1, message: '已核实该凭据仍存在：Error: delete failed' }])
  expect(f.store.getSnapshot().deleteProgress).toEqual({ current: 2, total: 2 })
  expect(f.store.batchDeleteLock.current).toBe(false)
})

test('quota failure reconciles credential and IP state before unlocking and allows retry', async () => {
  const f = fixture(), hold = gate<void>()
  f.api.quota = async () => { await hold.promise; throw Error('response lost') }
  const pending = f.mount().quota(); await tick()
  await f.mount().clear(); await f.mount().enable()
  expect(f.deleted).toEqual([]); expect(f.statusWrites).toEqual([])
  hold.resolve(); await pending
  expect(f.invalidated).toEqual(['credentials', 'credential-traffic', 'static-ips', 'proxy-pool', 'stats'])
  expect(f.store.batchDisableLock.current).toBe(false)
  expect(f.messages.at(-1)).toContain('response lost')
  f.api.quota = async () => ({ disabledIds: [3], skippedIds: [] }); await f.mount().quota()
  expect(f.counts().quotaCalls).toBe(2)
})

test('clear reconciles committed deletions after lost responses and removes absent selections', async () => {
  const f=fixture()
  f.api.remove=async id=>{f.server.splice(f.server.findIndex(c=>c.id===id),1);throw Error('lost response')}
  await f.mount().clear()
  expect(f.server.map(c=>c.id)).toEqual([3]);expect([...f.store.getSnapshot().selectedIds]).toEqual([])
  expect(f.store.getSnapshot().deleteErrors).toEqual([])
  expect(f.counts().reads).toBe(2);expect(f.messages.at(-1)).toContain('核实不存在 2')
})
test('clear can retry verification when no disabled accounts remain, without confirmation or more deletes', async () => {
  const f=fixture();let reads=0
  f.api.remove=async id=>{f.server.splice(f.server.findIndex(c=>c.id===id),1);throw Error('lost')}
  f.api.read=async()=>{if(++reads>1)throw Error('verification offline');return structuredClone(f.server)}
  await f.mount().clear()
  expect(f.store.getSnapshot().deleteErrors.every(e=>e.message.includes('结果未确认'))).toBe(true)
  f.store.setSelectedIds(current=>new Set([...current,99]))
  f.api.read=async()=>structuredClone(f.server)
  await f.mount().clear()
  expect(f.deleted).toEqual([1,2]);expect(f.counts().confirmations).toBe(1)
  expect([...f.store.getSnapshot().selectedIds]).toEqual([99]);expect(f.store.getSnapshot().deleteErrors).toEqual([])
  expect(f.store.batchDeleteLock.current).toBe(false)
})
test('clear retry retains errors and selections if the fresh verification still fails', async () => {
  const f=fixture();f.server.splice(0,2)
  f.store.setDeleteErrors([{id:1,message:'结果未确认'}]);f.store.setSelectedIds(new Set([1,99]))
  f.api.read=async()=>{throw Error('offline')}
  await f.mount().clear()
  expect(f.deleted).toEqual([]);expect([...f.store.getSnapshot().selectedIds]).toEqual([1,99])
  expect(f.store.getSnapshot().deleteErrors).toEqual([{id:1,message:'结果未确认'}])
  expect(f.store.batchDeleteLock.current).toBe(false)
})
test('verification-only clear preserves an enabled account failure and never adds it to deletion targets', async () => {
  const f=fixture();f.server.splice(0,2)
  f.store.setDeleteErrors([{id:3,message:'permission rejected'},{id:99,message:'结果未确认'}]);f.store.setSelectedIds(new Set([3,99]))
  await f.mount().clear()
  expect(f.deleted).toEqual([]);expect([...f.store.getSnapshot().selectedIds]).toEqual([3])
  expect(f.store.getSnapshot().deleteErrors).toEqual([{id:3,message:'permission rejected'}])
})
test('the clear menu allows verification when deletion errors remain but the disabled count is zero', () => {
  expect(source).toContain('(disabledCredentialCount === 0 && deleteErrors.length === 0)')
  expect(source).toContain('? "核对删除结果"')
})
