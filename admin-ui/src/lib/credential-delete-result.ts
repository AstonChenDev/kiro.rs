import { isAxiosError } from 'axios'

type DeleteError = { id: number; message: string }

/** Both selection deletion and disabled cleanup must settle lost responses alike. */
export async function deleteCredentialsWithVerification(ids: number[], api: {
  remove: (id: number) => Promise<unknown>
  read: () => Promise<{ credentials: { id: number }[] }>
  progress: (value: { current: number; total: number }) => void
  errorMessage: (error: unknown) => string
}) {
  const succeeded = new Set<number>()
  const failures: DeleteError[] = [], uncertain: DeleteError[] = []
  let unconfirmed = 0
  for (const [index, id] of ids.entries()) {
    try {
      await api.remove(id)
      succeeded.add(id)
    } catch (error) {
      const status = isAxiosError(error) ? error.response?.status : undefined
      const rejected = status && status >= 400 && status < 500 && status !== 404 && status !== 408
      const item = { id, message: api.errorMessage(error) }
      if (rejected) failures.push(item)
      else uncertain.push(item)
    }
    api.progress({ current: index + 1, total: ids.length })
  }
  if (uncertain.length) {
    try {
      const existing = new Set((await api.read()).credentials.map(c => c.id))
      for (const item of uncertain) {
        if (!existing.has(item.id)) succeeded.add(item.id)
        else failures.push({ id: item.id, message: `已核实该凭据仍存在：${item.message}` })
      }
    } catch {
      unconfirmed = uncertain.length
      failures.push(...uncertain.map(item => ({ id: item.id,
        message: `结果未确认，请恢复连接后核对删除状态：${item.message}` })))
    }
  }
  return { succeeded, failures, unconfirmed }
}
