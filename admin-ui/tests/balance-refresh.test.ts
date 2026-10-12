import { expect, test } from 'bun:test'
import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { BALANCE_REFRESH_KEY, createBalanceRefreshController } from '../src/lib/balance-refresh'
import type { RefreshJob } from '../src/api/traffic'

const idle: RefreshJob = { running: false, total: 0, completed: 0, failed: 0, skipped: 0, errors: {}, finishedAt: null }
const running: RefreshJob = { ...idle, running: true, total: 2 }
const finished: RefreshJob = { ...running, running: false, completed: 2, finishedAt: '2026-10-12T00:00:00Z' }
function deferred<T>() {
  let resolve!: (v: T) => void, reject!: (e: Error) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const clientForTest = () => new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } })

test('a late pre-start idle response cannot overwrite a new running job', async () => {
  const client = clientForTest(), oldRead = deferred<RefreshJob>(), entered = deferred<void>()
  let reads = 0
  const controller = createBalanceRefreshController(client, {
    start: async () => running,
    read: async () => { if (++reads === 1) { entered.resolve(); return oldRead.promise } return running },
  })
  const oldGet = client.fetchQuery({ queryKey: BALANCE_REFRESH_KEY, queryFn: controller.read }).catch(() => null)
  await entered.promise
  await controller.start()
  oldRead.resolve(idle); await oldGet
  expect(client.getQueryData(BALANCE_REFRESH_KEY)).toEqual(running)
  expect(await client.fetchQuery({ queryKey: BALANCE_REFRESH_KEY, queryFn: controller.read })).toEqual(running)
  client.clear()
})

test('queries created while POST is pending wait and read only the new server state', async () => {
  const client = clientForTest(), post = deferred<RefreshJob>(), entered = deferred<void>()
  let reads = 0
  const controller = createBalanceRefreshController(client, {
    start: () => { entered.resolve(); return post.promise },
    read: async () => { reads++; return finished },
  })
  const start = controller.start(); await entered.promise
  const duringStart = client.fetchQuery({ queryKey: BALANCE_REFRESH_KEY, queryFn: controller.read }).catch(() => null)
  await Promise.resolve(); expect(reads).toBe(0)
  post.resolve(running); await start; await duringStart
  expect(client.getQueryData(BALANCE_REFRESH_KEY)).toEqual(running)
  expect(await client.fetchQuery({ queryKey: BALANCE_REFRESH_KEY, queryFn: controller.read })).toEqual(finished)
  expect(reads).toBe(1)
  client.clear()
})

test('automatic, manual and remounted callers share one pending start', async () => {
  const client = clientForTest(), post = deferred<RefreshJob>()
  let posts = 0
  const api = { start: () => { posts++; return post.promise }, read: async () => running }
  const first = createBalanceRefreshController(client, api)
  const a = first.start(), b = first.start()
  const remounted = createBalanceRefreshController(client, api)
  const c = remounted.start()
  expect(a).toBe(b); expect(a).toBe(c)
  post.resolve(running); await Promise.all([a, b, c])
  expect(posts).toBe(1)
  client.clear()
})

test('a failed POST releases waiting reads and permits another start', async () => {
  const client = clientForTest(), post = deferred<RefreshJob>(), entered = deferred<void>()
  let posts = 0
  const controller = createBalanceRefreshController(client, {
    start: () => { posts++; entered.resolve(); return posts === 1 ? post.promise : Promise.resolve(running) },
    read: async () => running, // The server accepted the job but its response was lost.
  })
  const start = controller.start(); const failed = start.then(() => null, error => error)
  await entered.promise
  const read = controller.read({})
  post.reject(new Error('response lost')); expect((await failed).message).toBe('response lost')
  expect(await read).toEqual(running)
  expect(await controller.start()).toEqual(running)
  expect(posts).toBe(2)
  client.clear()
})

test('an active observer reconciles a job that finishes immediately after POST', async () => {
  const client = clientForTest(), completion = deferred<void>()
  let started = false
  const controller = createBalanceRefreshController(client, {
    start: async () => { started = true; return running },
    read: async () => started ? finished : idle,
  })
  const observer = new QueryObserver(client, {
    queryKey: BALANCE_REFRESH_KEY, queryFn: controller.read,
    refetchInterval: query => query.state.data?.running ? 2000 : false,
  })
  const unsubscribe = observer.subscribe(result => { if (result.data?.finishedAt) completion.resolve() })
  try {
    await controller.start(); await completion.promise
    expect(observer.getCurrentResult().data).toEqual(finished)
  } finally { unsubscribe(); client.clear() }
})

test('an aborted waiting query never starts an obsolete HTTP read', async () => {
  const client = clientForTest(), post = deferred<RefreshJob>(), abort = new AbortController()
  let reads = 0
  const controller = createBalanceRefreshController(client, { start: () => post.promise, read: async () => { reads++; return running } })
  const start = controller.start()
  const read = controller.read({ signal: abort.signal })
  const cancelled = read.then(() => null, error => error)
  abort.abort(); post.resolve(running); await start
  expect(await cancelled).toBeInstanceOf(Error)
  expect(reads).toBe(0)
  client.clear()
})
