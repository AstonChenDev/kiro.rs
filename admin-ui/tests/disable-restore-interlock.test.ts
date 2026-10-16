import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { isAxiosError } from 'axios'

const source = readFileSync(new URL('../src/components/dashboard.tsx', import.meta.url), 'utf8')
const extract = (start: string, end: string) => {
  const a = source.indexOf(start); const b = source.indexOf(end, a)
  if (a < 0 || b < 0) throw new Error('Batch handler missing')
  return source.slice(a, b)
}
const code = new Bun.Transpiler({ loader: 'tsx' }).transformSync(
  extract('  const handleBatchEnable =', '  const handleBatchDelete =') +
  extract('  const handleBatchResetFailure =', '  const handleBatchForceRefresh ='),
)

function fixture(options: { holdDisable?: boolean; rejectRestore?: boolean } = {}) {
  const server = [{ id: 1, disabled: true, failureCount: 1 }, { id: 2, disabled: false, failureCount: 1 }]
  const selected = new Set([1, 2]); const disableLock = { current: false }; const restoreLock = { current: false }
  let busyDisable = false; let busyRestore = false; let reads = 0; const writes: number[] = []; const restores: number[] = []
  let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve })
  const args = {
    enablePendingChecks: { current: new Set<number>() },
    selectedIds: selected, batchDisableLock: disableLock, batchRestoreLock: restoreLock, batchDeleteLock: { current: false }, isAxiosError,
    setBatchDisabling: (v: boolean) => { busyDisable = v }, setBatchRestoring: (v: boolean) => { busyRestore = v },
    setDisableErrors: () => {}, setDisableProgress: () => {},
    setBatchEnabling: () => {}, setEnableErrors: () => {}, setEnableProgress: () => {},
    getCredentials: async () => { reads++; return { credentials: structuredClone(server) } },
    getCredentialBalance: async () => ({}), setBalanceMap: () => {},
    getStaticIps: async () => ({ waitingCredentialIds: [] }),
    setCredentialDisabled: async (id: number, disabled: boolean) => {
      writes.push(id); if (options.holdDisable) await gate
      server.find(c => c.id === id)!.disabled = disabled
    },
    setSelectedIds: (update: (ids: Set<number>) => Set<number>) => {
      const next = update(selected); selected.clear(); next.forEach(id => selected.add(id))
    },
    extractErrorMessage: String, queryClient: { invalidateQueries: async () => {} },
    toast: { success: () => {}, warning: () => {}, error: () => {} },
    data: { credentials: structuredClone(server) }, deselectAll: () => selected.clear(),
    resetFailure: async (id: number) => {
      restores.push(id)
      await gate
      if (options.rejectRestore) throw new Error('rejected')
      server.find(c => c.id === id)!.disabled = false
    },
  }
  const handlers = new Function(...Object.keys(args), `${code};return {disable:handleBatchDisable,enable:handleBatchEnable,restore:handleBatchResetFailure}`)(...Object.values(args)) as {
    enable: () => Promise<void>; disable: () => Promise<void>; restore: () => Promise<void>
  }
  return { ...handlers, release, selectAll: () => { selected.add(1); selected.add(2) }, state: () => ({
    server, reads, writes, restores, busyDisable, busyRestore, disableLocked: disableLock.current, restoreLocked: restoreLock.current,
  }) }
}

test('pending restore blocks disable; disabling works normally after restore completes', async () => {
  const f = fixture(); const pending = f.restore()
  expect(f.state().restoreLocked).toBe(true)
  await f.disable(); expect(f.state().reads).toBe(0); expect(f.state().writes).toEqual([])
  f.release(); await pending
  expect(f.state()).toMatchObject({ restoreLocked: false, busyRestore: false })
  f.selectAll(); await f.disable()
  expect(f.state().server.every(c => c.disabled)).toBe(true)
  expect(f.state().writes).toEqual([1, 2])
})

test('pending disable blocks restore in the reverse order', async () => {
  const f = fixture({ holdDisable: true }); const pending = f.disable()
  await f.restore(); expect(f.state().restores).toEqual([])
  f.release(); await pending
  expect(f.state().server.every(c => c.disabled)).toBe(true)
  expect(f.state()).toMatchObject({ disableLocked: false, busyDisable: false })
})

test('duplicate restore clicks cannot launch additional in-flight restores', async () => {
  const f = fixture(); const first = f.restore(); await f.restore()
  expect(f.state().restores).toEqual([1])
  f.release(); await first; expect(f.state().restores).toEqual([1, 2])
})

test('restore failures release the interlock and permit a later disable', async () => {
  const f = fixture({ rejectRestore: true }); const pending = f.restore(); f.release(); await pending
  expect(f.state()).toMatchObject({ restoreLocked: false, busyRestore: false })
  f.selectAll(); await f.disable(); expect(f.state().server.every(c => c.disabled)).toBe(true)
})

test('pending enable blocks disable and restore until the enabled state is committed', async () => {
  const f = fixture({ holdDisable: true }); const pending = f.enable()
  await f.disable(); await f.restore()
  expect(f.state().restores).toEqual([])
  f.release(); await pending
  expect(f.state().writes).toEqual([1]); expect(f.state().server.every(c => !c.disabled)).toBe(true)
  expect(f.state().disableLocked).toBe(false)
})

test('pending disable blocks enable until the disabled state is committed', async () => {
  const f = fixture({ holdDisable: true }); const pending = f.disable()
  await f.enable(); f.release(); await pending
  expect(f.state().writes).toEqual([2]); expect(f.state().server.every(c => c.disabled)).toBe(true)
})
