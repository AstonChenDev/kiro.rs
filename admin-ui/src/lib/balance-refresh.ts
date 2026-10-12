import type { QueryClient } from '@tanstack/react-query'
import type { RefreshJob } from '@/api/traffic'

export const BALANCE_REFRESH_KEY = ['balance-refresh-job'] as const

interface BalanceRefreshController {
  start: () => Promise<RefreshJob>
  read: (context: { signal?: AbortSignal }) => Promise<RefreshJob>
}
const controllers = new WeakMap<QueryClient, BalanceRefreshController>()

/** Coordinate starts and reads so a pre-start snapshot cannot replace a new job. */
export function createBalanceRefreshController(client: QueryClient, api: {
  start: () => Promise<RefreshJob>
  read: (signal?: AbortSignal) => Promise<RefreshJob>
}): BalanceRefreshController {
  // A page remount must share any POST still pending for this query cache.
  const existing = controllers.get(client)
  if (existing) return existing
  let starting: Promise<RefreshJob> | null = null
  const filter = { queryKey: BALANCE_REFRESH_KEY, exact: true }

  const start = (): Promise<RefreshJob> => {
    if (starting) return starting
    starting = (async () => {
      try {
        await client.cancelQueries(filter)
        const job = await api.start()
        // Also cancel queries created by focus/remount while POST was pending.
        await client.cancelQueries(filter)
        client.setQueryData(BALANCE_REFRESH_KEY, job)
        return job
      } finally {
        starting = null
        // Reconcile even if POST failed: the server may already have started it.
        void client.invalidateQueries(filter)
      }
    })()
    return starting
  }

  const read = async ({ signal }: { signal?: AbortSignal }) => {
    if (starting) {
      try { await starting } catch { /* Read the actual server state after a failed start. */ }
    }
    signal?.throwIfAborted()
    return api.read(signal)
  }

  const controller = { start, read }
  controllers.set(client, controller)
  return controller
}
