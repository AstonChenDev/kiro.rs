import type { Dispatch, SetStateAction } from 'react'

type BatchError = { id: number; message: string }

function initialState() {
  return {
    selectedIds: new Set<number>(),
    batchDeleting: false,
    batchRestoring: false,
    batchDisabling: false,
    batchEnabling: false,
    singleChanging: false,
    enableProgress: { current: 0, total: 0 },
    batchQuotaReviewing: false,
    quotaProgress: { current: 0, total: 0 },
    quotaErrors: [] as BatchError[],
    quotaSummary: "",
    enableErrors: [] as BatchError[],
    disableProgress: { current: 0, total: 0 },
    disableErrors: [] as BatchError[],
    deleteProgress: { current: 0, total: 0 },
    deleteErrors: [] as BatchError[],
  }
}

/** One store per logged-in app: route unmounts must not reset an active batch. */
export function createCredentialBatchState() {
  let state = initialState()
  const listeners = new Set<() => void>()
  const batchDeleteLock = { current: false }
  const batchDisableLock = { current: false }
  const batchRestoreLock = { current: false }
  function setter<K extends keyof typeof state>(key: K): Dispatch<SetStateAction<(typeof state)[K]>> {
    return value => {
      const next = typeof value === 'function'
        ? (value as (previous: (typeof state)[K]) => (typeof state)[K])(state[key])
        : value
      if (Object.is(next, state[key])) return
      state = { ...state, [key]: next }
      listeners.forEach(listener => listener())
    }
  }
  return {
    getSnapshot: () => state,
    subscribe: (listener: () => void) => {
      listeners.add(listener)
      return () => { listeners.delete(listener) }
    },
    // Ref-shaped locks are synchronous, including before the next React render.
    batchDeleteLock,
    batchDisableLock,
    batchRestoreLock,
    // Card changes share the bulk status lock, including quota verification.
    // The app owns this promise/lock even if its originating card unmounts.
    runSingleCredentialChange: async (action: () => Promise<void>) => {
      if (batchDisableLock.current || batchDeleteLock.current || batchRestoreLock.current) return
      batchDisableLock.current = true
      setter('singleChanging')(true)
      try {
        await action()
      } finally {
        batchDisableLock.current = false
        setter('singleChanging')(false)
      }
    },
    setBatchQuotaReviewing: setter('batchQuotaReviewing'),
    setQuotaProgress: setter('quotaProgress'),
    setQuotaErrors: setter('quotaErrors'),
    setQuotaSummary: setter('quotaSummary'),
    setSelectedIds: setter('selectedIds'),
    setBatchDeleting: setter('batchDeleting'),
    setBatchRestoring: setter('batchRestoring'),
    setBatchDisabling: setter('batchDisabling'),
    setBatchEnabling: setter('batchEnabling'),
    setEnableProgress: setter('enableProgress'),
    setEnableErrors: setter('enableErrors'),
    setDisableProgress: setter('disableProgress'),
    setDisableErrors: setter('disableErrors'),
    setDeleteProgress: setter('deleteProgress'),
    setDeleteErrors: setter('deleteErrors'),
  }
}
