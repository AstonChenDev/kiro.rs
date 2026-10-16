import { createContext, useContext, useState, useSyncExternalStore, type ReactNode } from 'react'
import { createCredentialBatchState } from '@/lib/credential-batch-state'

const BatchContext = createContext<ReturnType<typeof createCredentialBatchState> | null>(null)

export function CredentialBatchProvider({ children }: { children: ReactNode }) {
  const [store] = useState(createCredentialBatchState)
  return <BatchContext.Provider value={store}>{children}</BatchContext.Provider>
}

export function useCredentialBatchState() {
  const store = useContext(BatchContext)
  if (!store) throw new Error('CredentialBatchProvider is required')
  const state = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot)
  return { ...store, ...state }
}

/** Cards only subscribe to busy state, not every selection/progress update. */
export function useCredentialStatusChange() {
  const store = useContext(BatchContext)
  if (!store) throw new Error('CredentialBatchProvider is required')
  const getBusy = () => {
    const state = store.getSnapshot()
    return state.batchQuotaReviewing || state.singleChanging || state.batchEnabling || state.batchDisabling || state.batchRestoring || state.batchDeleting
  }
  const statusChanging = useSyncExternalStore(store.subscribe, getBusy, getBusy)
  return { statusChanging, runSingleCredentialChange: store.runSingleCredentialChange }
}
