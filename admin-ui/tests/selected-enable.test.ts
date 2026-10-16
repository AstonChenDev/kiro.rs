import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { AxiosError, isAxiosError } from 'axios'
const source = readFileSync(new URL('../src/components/dashboard.tsx', import.meta.url), 'utf8')
const start = source.indexOf('  const handleBatchEnable = async () => {')
const end = source.indexOf('  const handleBatchDisable', start)
if (start < 0 || end < 0) throw new Error('Selected-enable handler missing')
const code = new Bun.Transpiler({ loader: 'tsx' }).transformSync(source.slice(start, end))
type Account = { id: number; disabled: boolean; disabledReason?: string; failureCount: number }
function fixture(options: {
  write?: (id: number, server: Account[]) => Promise<void>
  balance?: (id: number, server: Account[]) => Promise<unknown>
  read?: (count: number, server: Account[]) => Promise<Account[]>
  ips?: () => Promise<{ waitingCredentialIds: number[] }>
  blocked?: 'restore' | 'delete' | 'status'
} = {}) {
  const server: Account[] = [1, 42, 800].map(id => ({ id, disabled: id !== 42, failureCount: 0 }))
  let selected = new Set([1, 42, 800]), busy = false, errors: { id: number; message: string }[] = [], reads = 0
  const writes: number[] = [], balances: number[] = [], messages: string[] = [], invalidated: string[] = []
  let balanceMap = new Map(); const lock = { current: options.blocked === 'status' }
  const pending = { current: new Set<number>() }; const progress: any[] = []
  const args = {
    enablePendingChecks: pending,
    selectedIds: selected, batchDisableLock: lock, batchRestoreLock: { current: options.blocked === 'restore' }, batchDeleteLock: { current: options.blocked === 'delete' }, isAxiosError,
    getCredentials: async () => ({ credentials: structuredClone(options.read ? await options.read(++reads, server) : (reads++, server)) }),
    setCredentialDisabled: async (id: number, disabled: boolean) => {
      expect(disabled).toBe(false); writes.push(id)
      if (options.write) await options.write(id, server)
      else server.find(c => c.id === id)!.disabled = false
    },
    getCredentialBalance: async (id: number) => { balances.push(id); return options.balance ? options.balance(id, server) : { id, remaining: 10 } },
    getStaticIps: options.ips ?? (async () => ({ waitingCredentialIds: [] })),
    setBalanceMap: (update: (map: Map<number, unknown>) => Map<number, unknown>) => { balanceMap = update(balanceMap) },
    setBatchEnabling: (v: boolean) => { busy = v }, setEnableErrors: (v: typeof errors) => { errors = v }, setEnableProgress: (v: unknown) => { progress.push(v) },
    setSelectedIds: (update: (ids: Set<number>) => Set<number>) => { selected = update(selected) }, extractErrorMessage: String,
    queryClient: { invalidateQueries: async ({ queryKey }: { queryKey: string[] }) => { invalidated.push(queryKey[0]) } },
    toast: { success: (s: string) => messages.push(s), warning: (s: string) => messages.push(s), error: (s: string) => messages.push(s) },
  }
  const run = async () => {
    const currentArgs = { ...args, selectedIds: selected }
    await new Function(...Object.keys(currentArgs), `${code}; return handleBatchEnable`)(...Object.values(currentArgs))()
  }
  return { run, server, select: (id: number) => selected.add(id), state: () => ({ selected: [...selected], busy, locked: lock.current, errors, reads, writes, balances, messages, invalidated, balanceMap, progress, pending: [...pending.current] }) }
}
test('enable only writes disabled selections across pages and never queries quota or IP readiness', async () => {
  const f = fixture({ balance: async () => { throw Error('quota must not run') }, ips: async () => { throw Error('IP read must not run') } })
  await f.run()
  expect(f.state()).toMatchObject({ writes: [1, 800], balances: [], selected: [], busy: false, locked: false, errors: [] })
  expect(f.state().balanceMap.size).toBe(0)
  expect(f.state().messages[0]).toContain('已启用 2 个，已启用跳过 1 个')
  expect(f.state().progress.at(-1)).toEqual({ current: 3, total: 3 })
})
test('an exhausted account is enabled without triggering automatic quota recheck', async () => {
  const f = fixture({ balance: async (_id, server) => { server[0].disabled = true; throw Error('quota exhausted') } })
  f.server[0].disabledReason = 'QuotaExceeded'
  await f.run()
  expect(f.server[0].disabled).toBe(false); expect(f.state().balances).toEqual([])
})
test('waiting for a static IP does not make an enable write fail', async () => {
  const f = fixture({ ips: async () => ({ waitingCredentialIds: [1, 800] }) })
  await f.run(); expect(f.state().errors).toEqual([]); expect(f.state().selected).toEqual([])
})
test('lost committed responses are reconciled using account state only', async () => {
  const f = fixture({ write: async (id, server) => { server.find(c => c.id === id)!.disabled = false; throw Error('lost') } })
  await f.run(); expect(f.state()).toMatchObject({ selected: [], balances: [], errors: [], reads: 2 })
})
test('uncommitted writes remain selected with the correct disabled-state reason', async () => {
  const f = fixture({ write: async () => { throw Error('offline') } })
  await f.run(); expect(f.state().selected).toEqual([1, 800]); expect(f.state().errors[0].message).toContain('仍禁用')
})
test('uncertain responses stay selected, and retry checks state without repeating successful writes', async () => {
  let offline = true
  const f = fixture({ write: async (id, server) => { server.find(c => c.id === id)!.disabled = false; throw Error('lost') },
    read: async (n, server) => { if (n > 1 && offline) throw Error('offline'); return server } })
  await f.run(); expect(f.state().selected).toEqual([1, 800]); expect(f.state().messages[0]).toContain('2 个结果未确认')
  offline = false; await f.run()
  expect(f.state().writes).toEqual([1, 800]); expect(f.state().balances).toEqual([]); expect(f.state().selected).toEqual([])
})
test('explicit rejection preserves the reason while other selected accounts continue', async () => {
  const f = fixture({ write: async (id, server) => {
    if (id === 1) throw new AxiosError('forbidden', 'ERR_BAD_REQUEST', undefined, undefined, { status: 403 } as never)
    server.find(c => c.id === id)!.disabled = false
  } })
  await f.run(); expect(f.state().selected).toEqual([1]); expect(f.state().errors[0].message).toContain('forbidden')
})
test('missing selections report an error without blocking other accounts', async () => {
  const f = fixture(); f.server.splice(0, 1); await f.run()
  expect(f.state().writes).toEqual([800]); expect(f.state().errors[0].message).toContain('不存在')
})
test('duplicate clicks are blocked during writes and new selections are preserved', async () => {
  let release!: () => void; const gate = new Promise<void>(r => { release = r })
  const f = fixture({ write: async (id, server) => { await gate; server.find(c => c.id === id)!.disabled = false } })
  const running = f.run(); for (let i = 0; i < 10; i++) await Promise.resolve()
  await f.run(); f.select(999); release(); await running
  expect(f.state().writes).toEqual([1, 800]); expect(f.state().selected).toEqual([999]); expect(f.state().locked).toBe(false)
})
for (const blocked of ['restore', 'delete', 'status'] as const) test(`enable respects ${blocked} lock`, async () => {
  const f = fixture({ blocked }); await f.run(); expect(f.state().writes).toEqual([])
})
test('initial status failure performs no writes and unlocks', async () => {
  const f = fixture({ read: async () => { throw Error('offline') } }); await f.run()
  expect(f.state()).toMatchObject({ writes: [], balances: [], busy: false, locked: false, selected: [1, 42, 800] })
})
