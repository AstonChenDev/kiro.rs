import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
const source = readFileSync(new URL('../src/components/dashboard.tsx', import.meta.url), 'utf8')
const start = source.indexOf('  const handleBatchDelete = async () => {')
const end = source.indexOf('  const handleBatchResetFailure', start)
if (start < 0 || end < 0) throw new Error('Selected-delete handler missing')
const code = new Bun.Transpiler({ loader: 'tsx' }).transformSync(source.slice(start, end))
function fixture(remove: (id: number) => Promise<void>, confirm: () => Promise<boolean> = async () => true) {
  let selected = new Set([1, 42, 800]); let busy = false
  let failures: { id: number; message: string }[] = []
  const lock = { current: false }; const messages: string[] = []; const invalidate: string[] = []
  const args = {
    selectedIds: selected, batchDeleteLock: lock, confirm, deleteCredentialRequest: remove,
    setBatchDeleting: (value: boolean) => { busy = value },
    setDeleteErrors: (value: typeof failures) => { failures = value }, setDeleteProgress: () => {},
    setSelectedIds: (update: (old: Set<number>) => Set<number>) => { selected = update(selected) },
    extractErrorMessage: String,
    queryClient: { invalidateQueries: async ({ queryKey }: { queryKey: string[] }) => { invalidate.push(queryKey[0]) } },
    toast: { success: (m: string) => messages.push(m), warning: (m: string) => messages.push(m), error: (m: string) => messages.push(m) },
  }
  const run = new Function(...Object.keys(args), `${code}; return handleBatchDelete;`)(...Object.values(args)) as () => Promise<void>
  return { run, state: () => ({ selected: [...selected], busy, locked: lock.current, failures, invalidate, messages }) }
}
test('deletes all selected IDs, including selections from other pages', async () => {
  const ids: number[] = []; const f = fixture(async id => { ids.push(id) })
  await f.run(); expect(ids).toEqual([1, 42, 800])
  expect(f.state()).toMatchObject({ selected: [], busy: false, locked: false, failures: [], invalidate: ['credentials', 'credential-traffic', 'stats'] })
})
test('continues after a failure and retains only failed selections', async () => {
  const ids: number[] = []; const f = fixture(async id => { ids.push(id); if (id === 42) throw new Error('injected failure') })
  await f.run(); expect(ids).toEqual([1, 42, 800])
  expect(f.state()).toMatchObject({ selected: [42], busy: false, locked: false })
  expect(f.state().failures[0].message).toContain('injected failure')
})
test('cancellation leaves selections untouched', async () => {
  let calls = 0; const f = fixture(async () => { calls++ }, async () => false)
  await f.run(); expect(calls).toBe(0)
  expect(f.state()).toMatchObject({ selected: [1, 42, 800], busy: false, locked: false })
})
test('duplicate clicks cannot issue duplicate deletes', async () => {
  let release!: () => void; const waiting = new Promise<void>(resolve => { release = resolve })
  const ids: number[] = []; const f = fixture(async id => { ids.push(id); if (id === 1) await waiting })
  const first = f.run(); await f.run(); expect(ids).toEqual([1])
  release(); await first; expect(ids).toEqual([1, 42, 800]); expect(f.state().locked).toBe(false)
})
