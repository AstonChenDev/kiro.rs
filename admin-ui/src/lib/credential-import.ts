/** Imported subscription is a cached title; it never creates a manual traffic-tier override. */
export function importedSubscriptionTitle(account: Record<string, unknown>): string | undefined {
  if (typeof account.subscriptionTitle === 'string' && account.subscriptionTitle.trim()) return account.subscriptionTitle.trim()
  const subscription = account.subscription
  if (!subscription || typeof subscription !== 'object' || Array.isArray(subscription)) return undefined
  const { title, type } = subscription as Record<string, unknown>
  if (typeof title === 'string' && title.trim()) return title.trim()
  const titles: Record<string, string> = { FREE: 'KIRO FREE', PRO: 'KIRO PRO', PRO_PLUS: 'KIRO PRO+', 'PRO+': 'KIRO PRO+', MAX: 'KIRO PRO MAX', POWER: 'KIRO POWER', STUDENT: 'KIRO STUDENT' }
  return typeof type === 'string' ? titles[type.trim().toUpperCase()] : undefined
}

export function normalizeImportExpiresAt(value: unknown): string | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) {
    const date = new Date(value)
    return Number.isNaN(date.getTime()) ? undefined : date.toISOString()
  }
  return typeof value === 'string' ? value.trim() || undefined : undefined
}

/** Accept ordinary credentials and KAM exports in the common batch-import entry. */
export function parseCredentialImport(raw: string): Record<string, unknown>[] {
  const input = raw.trim()
  let parsed: unknown
  try {
    parsed = JSON.parse(input)
  } catch {
    // Accept copied object lists: {...}, {...}. Keep JSON's strict grammar;
    // never repair token strings, evaluate code, or expose input in errors.
    try {
      if (!input.startsWith('{') || !input.endsWith('}')) throw new Error()
      parsed = JSON.parse(`[${input}]`)
    } catch {
      throw new Error('请提供 JSON 数组、单个凭据对象，或用逗号分隔的凭据对象')
    }
  }
  const isObject = (value: unknown): value is Record<string, unknown> =>
    value !== null && typeof value === 'object' && !Array.isArray(value)
  const items = isObject(parsed) && 'accounts' in parsed
    ? parsed.accounts
    : Array.isArray(parsed) ? parsed : [parsed]
  if (!Array.isArray(items)) throw new Error('accounts 必须为账号数组')
  return items.map((item, index) => {
    if (!isObject(item)) throw new Error(`第 ${index + 1} 条凭据必须为对象`)
    if (!('credentials' in item)) return { ...item, subscriptionTitle: importedSubscriptionTitle(item) }
    if (!isObject(item.credentials)) throw new Error(`第 ${index + 1} 条 credentials 必须为对象`)
    const credentials = item.credentials
    return {
      ...item,
      ...credentials,
      subscriptionTitle: importedSubscriptionTitle(item) ?? importedSubscriptionTitle(credentials),
      email: item.email ?? credentials.email,
      machineId: item.machineId ?? credentials.machineId,
      traffic: item.traffic ?? credentials.traffic,
      provider: typeof credentials.provider === 'string' && credentials.provider.trim()
        ? credentials.provider : item.idp ?? item.provider,
    }
  })
}
