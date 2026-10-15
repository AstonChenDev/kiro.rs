import { expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'

const source = readFileSync(new URL('../src/components/static-ip-dialog.tsx', import.meta.url), 'utf8')
const start = source.indexOf('  async function readLatestPool()')
const end = source.indexOf('  function add()', start)
if (start < 0 || end < 0) throw new Error('Static IP policy handlers missing')
const code = new Bun.Transpiler({ loader: 'tsx' }).transformSync(source.slice(start, end))

function fixture() {
  let server = ['github', 'google']; let cached = [...server]
  let reads = 0; let failedReads = false; let loseResponse = false; let rejectWrite = false
  let holdRead: Promise<void> | undefined; let busy = false; let needsReload = false
  const lock = { current: false }, recoveryRequired = { current: false }
  const writes: string[][] = []; const invalidations: string[] = []; const messages: string[] = []
  const args = {
    pool: { refetch: async (options: { cancelRefetch: boolean }) => {
      expect(options.cancelRefetch).toBe(true); reads++
      if (holdRead) await holdRead
      if (failedReads) return { isError: true, error: new Error('read unavailable'), data: { allowedProviders: cached } }
      cached = [...server]; return { isError: false, data: { allowedProviders: [...server] } }
    } },
    lock, recoveryRequired, setNeedsReload: (value: boolean) => { needsReload = value },
    setBusy: (value: boolean) => { busy = value },
    setStaticIpProviders: async (providers: string[]) => {
      writes.push([...providers]); if (rejectWrite) throw new Error('受影响账号仍有请求进行中')
      server = [...providers]; if (loseResponse) throw new Error('response lost after commit')
    },
    toast: { success: (s: string) => messages.push(s), warning: (s: string) => messages.push(s) },
    extractErrorMessage: String,
    client: { invalidateQueries: async ({ queryKey }: { queryKey: string[] }) => { invalidations.push(queryKey[0]) } },
  }
  const handlers = new Function(...Object.keys(args), `${code}; return {changeProvider, run, retryReload};`)(...Object.values(args)) as {
    changeProvider: (value: string, enabled: boolean) => Promise<void>
    run: (action: () => Promise<unknown>, message: string, clear?: () => void) => Promise<void>
    retryReload: () => Promise<void>
  }
  return { ...handlers,
    options: (o: { failedReads?: boolean; loseResponse?: boolean; rejectWrite?: boolean; holdRead?: Promise<void> }) => {
      if (o.failedReads !== undefined) failedReads = o.failedReads
      if (o.loseResponse !== undefined) loseResponse = o.loseResponse
      if (o.rejectWrite !== undefined) rejectWrite = o.rejectWrite
      holdRead = o.holdRead
    },
    externalProviders: (providers: string[]) => { server = providers },
    state: () => ({ server, cached, reads, writes, busy, needsReload, locked: lock.current, recoveryRequired: recoveryRequired.current, invalidations, messages }),
  }
}

test('lost provider response followed by another toggle cannot restore the removed provider', async () => {
  const f = fixture(); f.options({ loseResponse: true })
  await f.changeProvider('github', false)
  expect(f.state()).toMatchObject({ server: ['google'], cached: ['google'], needsReload: false, busy: false })
  f.options({ loseResponse: false }); await f.changeProvider('google', false)
  expect(f.state().server).toEqual([]); expect(f.state().writes).toEqual([['google'], []])
})

test('provider update uses fresh server state even when the render cache is old', async () => {
  const f = fixture(); f.externalProviders(['google', 'builderid'])
  await f.changeProvider('google', false)
  expect(f.state().server).toEqual(['builderid'])
})

test('read failure with retained stale data blocks writes until explicit recovery succeeds', async () => {
  const f = fixture(); f.options({ failedReads: true })
  await f.changeProvider('github', false)
  expect(f.state()).toMatchObject({ writes: [], recoveryRequired: true, needsReload: true, busy: false })
  let called = false; await f.run(async () => { called = true }, 'should not run')
  expect(called).toBe(false)
  f.options({ failedReads: false }); await f.changeProvider('google', false)
  expect(f.state().writes).toEqual([])
  await f.retryReload(); expect(f.state()).toMatchObject({ recoveryRequired: false, needsReload: false, locked: false })
  await f.changeProvider('github', false); expect(f.state().server).toEqual(['google'])
})

test('committed write with lost response and failed verification remains guarded through failed retries', async () => {
  const f = fixture()
  await f.run(async () => {
    f.externalProviders(['google']); f.options({ failedReads: true }); throw new Error('response lost')
  }, 'saved')
  expect(f.state()).toMatchObject({ server: ['google'], cached: ['github', 'google'], recoveryRequired: true })
  await f.retryReload(); expect(f.state()).toMatchObject({ needsReload: true, busy: false })
  f.options({ failedReads: false }); await f.retryReload()
  await f.changeProvider('google', false); expect(f.state().server).toEqual([])
})

test('successful write still requires recovery when the following read fails', async () => {
  const f = fixture(); let cleared = false
  await f.run(async () => { f.externalProviders(['google']); f.options({ failedReads: true }) }, 'saved', () => { cleared = true })
  expect(cleared).toBe(true); expect(f.state()).toMatchObject({ recoveryRequired: true, needsReload: true })
})

test('explicit rejected policy preserves current values and reason without reporting success', async () => {
  const f = fixture(); f.options({ rejectWrite: true })
  await f.changeProvider('github', false)
  expect(f.state()).toMatchObject({ server: ['github', 'google'], recoveryRequired: false, busy: false })
  expect(f.state().messages.some(s => s.includes('受影响账号仍有请求进行中'))).toBe(true)
  expect(f.state().messages).not.toContain('账号类型策略已保存')
})

test('duplicate toggles and recovery clicks cannot start a second operation while a read is pending', async () => {
  const f = fixture(); let release!: () => void
  const pendingRead = new Promise<void>(resolve => { release = resolve }); f.options({ holdRead: pendingRead })
  const first = f.changeProvider('github', false)
  await f.changeProvider('google', false); await f.retryReload()
  expect(f.state()).toMatchObject({ reads: 1, busy: true, writes: [] })
  release(); await first; expect(f.state().writes).toEqual([['google']]); expect(f.state().locked).toBe(false)
})

test('ordinary successful operations retain clearing and dependent-query refresh behavior', async () => {
  const f = fixture(); let cleared = 0
  await f.run(async () => {}, 'added', () => { cleared++ })
  expect(cleared).toBe(1); expect(f.state().messages).toContain('added')
  expect(f.state().invalidations).toEqual(['credentials', 'proxy-pool'])
  expect(f.state()).toMatchObject({ needsReload: false, busy: false, locked: false })
})
