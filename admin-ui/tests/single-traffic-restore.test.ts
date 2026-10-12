import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import type { CredentialTraffic } from '../src/api/traffic'

const source = readFileSync(new URL('../src/components/traffic-controls.tsx', import.meta.url), 'utf8')
const start = source.indexOf('  const actionLock = useRef(false)', source.indexOf('export function CredentialTrafficControls'))
const end = source.indexOf('  if (!row)', start)
if (start < 0 || end < 0) throw new Error('Single-account submission code missing')
const code = new Bun.Transpiler({ loader: 'tsx' }).transformSync(source.slice(start, end))
const restoreClick = source.match(/onClick=\{(\(\) => submit\(\{ kind: 'restore' \}\))\}/)?.[1]
if (!restoreClick) throw new Error('Restore button missing')
type Action = { kind: 'save'; settings: CredentialTraffic } | { kind: 'restore' }
function fixture(wait: () => Promise<void> = async () => {}) {
  const lock = { current: false }, calls: { method: string; id: number; settings?: CredentialTraffic }[] = []
  let server: CredentialTraffic = { tier: 'student', concurrency: 150, rpm: 100 }, editing = true
  const errors: string[] = []
  const args = {
    id: 7, useRef: () => lock,
    // Execute the component's actual mutation callbacks, without rendering DOM.
    useMutation: (options: any) => ({ mutateAsync: async (action: Action) => {
      try { await options.mutationFn(action); await options.onSuccess() }
      catch (error) { options.onError(error); throw error }
    } }),
    restoreCredentialTrafficLimits: async (id: number) => { calls.push({ method: 'restore', id }); await wait(); server = { ...server, concurrency: null, rpm: null } },
    saveCredentialTraffic: async (id: number, settings: CredentialTraffic) => { calls.push({ method: 'put', id, settings }); await wait(); server = { ...settings } },
    client: { invalidateQueries: async () => {} }, setEditing: (v: boolean) => { editing = v },
    toast: { success: () => {}, error: (message: string) => errors.push(message) }, extractErrorMessage: String,
  }
  const actions = new Function(...Object.keys(args), `${code}; return { submit, restore: ${restoreClick} };`)(...Object.values(args)) as {
    submit: (action: Action) => Promise<void>; restore: () => Promise<void>
  }
  return { ...actions, state: () => ({ server, editing, locked: lock.current, calls, errors }) }
}

test('the single restore button uses only the ID and preserves a newly selected tier', async () => {
  const f = fixture()
  await f.restore()
  expect(f.state()).toMatchObject({ server: { tier: 'student', concurrency: null, rpm: null }, calls: [{ method: 'restore', id: 7 }], editing: false, locked: false })
})

test('ordinary saving still sends all explicit account settings', async () => {
  const f = fixture(), settings: CredentialTraffic = { tier: 'power', concurrency: 150, rpm: 100 }
  await f.submit({ kind: 'save', settings })
  expect(f.state()).toMatchObject({ server: settings, calls: [{ method: 'put', id: 7, settings }] })
})

test('save and restore cannot overlap in either direction', async () => {
  for (const first of ['save', 'restore'] as const) {
    let release!: () => void
    const waiting = new Promise<void>(resolve => { release = resolve })
    const f = fixture(() => waiting)
    const save: Action = { kind: 'save', settings: { tier: 'power', concurrency: 150, rpm: 100 } }
    const job = first === 'save' ? f.submit(save) : f.restore()
    await f.restore(); await f.submit(save)
    expect(f.state().locked).toBe(true)
    expect(f.state().calls).toHaveLength(1)
    release(); await job
    expect(f.state().locked).toBe(false)
  }
})

test('restore failure keeps the editor open and releases the lock for retry', async () => {
  let fail = true
  const f = fixture(async () => { if (fail) throw new Error('storage failed') })
  await f.restore()
  expect(f.state()).toMatchObject({ editing: true, locked: false, server: { tier: 'student', concurrency: 150, rpm: 100 } })
  expect(f.state().errors).toHaveLength(1)
  fail = false; await f.restore()
  expect(f.state()).toMatchObject({ editing: false, locked: false, server: { tier: 'student', concurrency: null, rpm: null } })
})
