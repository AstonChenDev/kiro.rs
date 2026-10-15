import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { AxiosError, isAxiosError } from 'axios'

const source = readFileSync(new URL('../src/components/dashboard.tsx', import.meta.url), 'utf8')
const start = source.indexOf('  const handleBatchDisable = async () => {')
const end = source.indexOf('  const handleBatchDelete', start)
if (start < 0 || end < 0) throw new Error('Selected-disable handler missing')
const code = new Bun.Transpiler({ loader: 'tsx' }).transformSync(source.slice(start, end))

function fixture(write: (id: number, disabled: boolean) => Promise<void>, options: {
  read?: () => Promise<{ credentials: { id: number; disabled: boolean }[] }>
  deleting?: boolean
} = {}) {
  let selected = new Set([1, 42, 800]); let busy = false
  let failures: { id: number; message: string }[] = []
  let progress = { current: 0, total: 0 }
  const lock = { current: false }; const invalidated: string[] = []; const messages: string[] = []
  const args = {
    selectedIds: selected, batchDisableLock: lock, batchRestoreLock: { current: false }, batchDeleteLock: { current: options.deleting ?? false }, isAxiosError,
    getCredentials: options.read ?? (async () => ({ credentials: [
      { id: 1, disabled: false }, { id: 42, disabled: true }, { id: 800, disabled: false },
    ] })),
    setCredentialDisabled: write,
    setBatchDisabling: (value: boolean) => { busy = value },
    setDisableErrors: (value: typeof failures) => { failures = value },
    setDisableProgress: (value: typeof progress) => { progress = value },
    setSelectedIds: (update: (old: Set<number>) => Set<number>) => { selected = update(selected) },
    extractErrorMessage: String,
    queryClient: { invalidateQueries: async ({ queryKey }: { queryKey: string[] }) => { invalidated.push(queryKey[0]) } },
    toast: { success: (s: string) => messages.push(s), warning: (s: string) => messages.push(s), error: (s: string) => messages.push(s) },
  }
  const run = new Function(...Object.keys(args), `${code}; return handleBatchDisable;`)(...Object.values(args)) as () => Promise<void>
  return { run, addSelection: (id: number) => selected.add(id), state: () => ({ selected: [...selected], busy, locked: lock.current, failures, progress, invalidated, messages }) }
}

test('disables all selected pages, skips already-disabled accounts and refreshes occupancy', async () => {
  const calls: [number, boolean][] = []; const f = fixture(async (id, disabled) => { calls.push([id, disabled]) })
  await f.run()
  expect(calls).toEqual([[1, true], [800, true]])
  expect(f.state()).toMatchObject({ selected: [], busy: false, locked: false, failures: [], progress: { current: 3, total: 3 } })
  expect(f.state().invalidated).toContain('static-ips')
  expect(f.state().messages[0]).toContain('已禁用 2 个，已禁用跳过 1 个')
})

test('one failure does not stop remaining accounts and remains selected for retry', async () => {
  const calls: number[] = []; const f = fixture(async id => { calls.push(id); if (id === 1) throw new Error('denied') })
  await f.run()
  expect(calls).toEqual([1, 800]); expect(f.state().selected).toEqual([1])
  expect(f.state().failures).toEqual([{ id: 1, message: '已核实该账号当前仍启用：Error: denied' }])
  expect(f.state().locked).toBe(false)
})

test('duplicate clicks cannot issue duplicate disables while initial status read is pending', async () => {
  let release!: () => void; const waiting = new Promise<void>(resolve => { release = resolve })
  let reads = 0; const calls: number[] = []
  const f = fixture(async id => { calls.push(id) }, { read: async () => {
    reads++; await waiting; return { credentials: [1, 42, 800].map(id => ({ id, disabled: false })) }
  } })
  const first = f.run(); await f.run(); expect(reads).toBe(1); expect(f.state().busy).toBe(true)
  release(); await first; expect(calls).toEqual([1, 42, 800]); expect(f.state().locked).toBe(false)
})

test('status read failure performs no writes and retains selections', async () => {
  let writes = 0
  const f = fixture(async () => { writes++ }, { read: async () => { throw new Error('offline') } })
  await f.run(); expect(writes).toBe(0)
  expect(f.state()).toMatchObject({ selected: [1, 42, 800], locked: false, busy: false })
  expect(f.state().messages[0]).toContain('未执行批量禁用')
})

test('new selections made during the batch are not disabled or cleared', async () => {
  let release!: () => void; const waiting = new Promise<void>(resolve => { release = resolve })
  const calls: number[] = []; const f = fixture(async id => { calls.push(id); if (id === 1) await waiting })
  const first = f.run(); f.addSelection(999); release(); await first
  expect(calls).toEqual([1, 800]); expect(f.state().selected).toEqual([999])
})

test('missing selected credential is reported while other selections still finish', async () => {
  const calls: number[] = []
  const f = fixture(async id => { calls.push(id) }, { read: async () => ({ credentials: [{ id: 800, disabled: false }] }) })
  await f.run(); expect(calls).toEqual([800]); expect(f.state().selected).toEqual([1, 42])
  expect(f.state().failures).toHaveLength(2)
})

test('bulk deletion lock prevents overlapping batch disable', async () => {
  let writes = 0; const f = fixture(async () => { writes++ }, { deleting: true })
  await f.run(); expect(writes).toBe(0); expect(f.state().selected).toEqual([1, 42, 800])
})

test('lost responses are counted as completed only after a fresh read confirms disabled', async () => {
  const server = [1, 42, 800].map(id => ({ id, disabled: id === 42 }))
  let reads = 0; const calls: number[] = []
  const f = fixture(async id => {
    calls.push(id); server.find(c => c.id === id)!.disabled = true
    throw new Error('response lost')
  }, { read: async () => { reads++; return { credentials: structuredClone(server) } } })
  await f.run()
  expect(reads).toBe(2); expect(calls).toEqual([1, 800])
  expect(f.state()).toMatchObject({ selected: [], failures: [], busy: false, locked: false })
  expect(f.state().messages[0]).toContain('已禁用 2 个，已禁用跳过 1 个')
})

test('failed verification labels results unconfirmed and retains their selections', async () => {
  let reads = 0
  const f = fixture(async () => { throw new Error('timeout') }, { read: async () => {
    if (++reads > 1) throw new Error('offline')
    return { credentials: [1, 42, 800].map(id => ({ id, disabled: id === 42 })) }
  } })
  await f.run()
  expect(f.state().selected).toEqual([1, 800])
  expect(f.state().failures.every(f => f.message.startsWith('结果未确认'))).toBe(true)
  expect(f.state().messages[0]).toContain('2 个结果未确认')
  expect(f.state().locked).toBe(false)
})

test('verification separates successful writes from accounts still enabled', async () => {
  let reads = 0
  const f = fixture(async () => { throw new Error('response lost') }, { read: async () => {
    reads++
    return { credentials: [1, 42, 800].map(id => ({ id, disabled: id === 42 || (reads > 1 && id === 1) })) }
  } })
  await f.run(); expect(f.state().selected).toEqual([800])
  expect(f.state().failures[0].message).toContain('当前仍启用')
  expect(f.state().messages[0]).toContain('已禁用 1 个，已禁用跳过 1 个，1 个未完成')
})

test('explicit server rejection keeps the failure reason without treating it as an uncertain write', async () => {
  let reads = 0
  const f = fixture(async id => {
    if (id === 1) throw new AxiosError('forbidden', 'ERR_BAD_REQUEST', undefined, undefined,
      { status: 403 } as never)
  }, { read: async () => { reads++; return { credentials: [1, 42, 800].map(id => ({ id, disabled: id === 42 })) } } })
  await f.run(); expect(reads).toBe(1); expect(f.state().selected).toEqual([1])
  expect(f.state().failures[0].message).toContain('forbidden')
})
