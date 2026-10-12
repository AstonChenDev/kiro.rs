import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'

const source = readFileSync(new URL('../src/components/batch-edit-credential-dialog.tsx', import.meta.url), 'utf8')
const start = source.indexOf('  const handleApply = async () => {')
const end = source.indexOf('\n  return (', start)
if (start < 0 || end < 0) throw new Error('Batch-edit handler missing')
const code = new Bun.Transpiler({ loader: 'tsx' }).transformSync(source.slice(start, end))
function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>(r => { resolve = r })
  return { promise, resolve }
}
function fixture(restore: (id: number) => Promise<void>, options: { editSource?: boolean; invalidate?: () => Promise<void> } = {}) {
  let running = false
  const applyLock = { current: false }
  const restored: number[] = [], edits: unknown[] = [], messages: string[] = []
  const args = {
    applyLock, restoreTraffic: true, editGroups: false, editSource: options.editSource ?? false,
    editType: false, editSaleStatus: false, editSalePrice: false, salePrice: '', sourceChannel: ' new source ',
    credentials: [{ id: 1 }, { id: 2 }],
    setRunning: (v: boolean) => { running = v }, setProgress: () => {},
    restoreCredentialTrafficLimits: async (id: number) => { restored.push(id); await restore(id) },
    updateCredential: async (id: number, req: unknown) => { edits.push({ id, req }) },
    queryClient: { invalidateQueries: options.invalidate ?? (async () => {}) },
    toast: Object.fromEntries(['success', 'warning', 'error'].map(key => [key, (m: string) => messages.push(m)])),
    onOpenChange: () => {}, onDone: () => {},
  }
  const run = new Function(...Object.keys(args), `${code}; return handleApply;`)(...Object.values(args)) as () => Promise<void>
  return { run, state: () => ({ running, locked: applyLock.current, restored, edits, messages }) }
}

test('restore locks before the first request and rejects duplicate submissions', async () => {
  const waiting = deferred()
  const f = fixture(async id => { if (id === 1) await waiting.promise })
  const first = f.run()
  expect(f.state()).toMatchObject({ running: true, locked: true, restored: [1] })
  await f.run()
  expect(f.state().restored).toEqual([1])
  waiting.resolve(); await first
  expect(f.state()).toMatchObject({ running: false, locked: false, restored: [1, 2], edits: [] })
})

test('restore sends only IDs so another editor can change the next account tier', async () => {
  const waiting = deferred(), entered = deferred()
  const server = new Map([1, 2].map(id => [id, { tier: 'power', concurrency: 150 as number | null, rpm: 100 as number | null }]))
  const f = fixture(async id => {
    if (id === 1) { entered.resolve(); await waiting.promise }
    const current = server.get(id)!
    server.set(id, { ...current, concurrency: null, rpm: null })
  })
  const job = f.run(); await entered.promise
  server.get(2)!.tier = 'student'
  waiting.resolve(); await job
  expect(server.get(2)).toEqual({ tier: 'student', concurrency: null, rpm: null })
  expect(f.state().restored).toEqual([1, 2])
})

test('one failed restore does not prevent other accounts and releases the lock', async () => {
  const f = fixture(async id => { if (id === 1) throw new Error('storage failure') })
  await f.run()
  expect(f.state()).toMatchObject({ running: false, locked: false, restored: [1, 2], messages: ['成功 1 个，失败 1 个'] })
})

test('query refresh errors release the lock so the user can retry', async () => {
  const f = fixture(async () => {}, { invalidate: async () => { throw new Error('refresh failed') } })
  await f.run()
  expect(f.state()).toMatchObject({ running: false, locked: false, restored: [1, 2] })
  expect(f.state().messages[0]).toContain('请刷新后检查账号状态')
  await f.run()
  expect(f.state().restored).toEqual([1, 2, 1, 2])
})

test('combining restore with an existing edit preserves the original field payload', async () => {
  const f = fixture(async () => {}, { editSource: true })
  await f.run()
  expect(f.state().edits).toEqual([1, 2].map(id => ({ id, req: { sourceChannel: 'new source' } })))
  expect(f.state().restored).toEqual([1, 2])
})
