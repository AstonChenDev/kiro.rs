import { importedSubscriptionTitle } from './credential-import'
// 兼容 KAM 1.8.3 新版平铺格式，统一转换为旧格式（credentials 嵌套结构）
export function normalizeKamAccount(item: unknown): unknown {
  if (typeof item !== 'object' || item === null) return item
  const obj = item as Record<string, unknown>
  // 新格式：refreshToken 直接在账号对象上，无 credentials 嵌套
  if (typeof obj.refreshToken === 'string' && typeof obj.credentials === 'undefined') {
    const email = typeof obj.email === 'string' ? obj.email : undefined
    const userId =
      typeof obj.userId === 'string' || obj.userId === null ? (obj.userId as string | null) : undefined
    const nickname =
      typeof obj.nickname === 'string'
        ? obj.nickname
        : typeof obj.label === 'string'
          ? (obj.label as string)
          : undefined
    const status = typeof obj.status === 'string' ? obj.status : undefined
    const idp = typeof obj.idp === 'string' ? obj.idp : undefined
    const machineId = typeof obj.machineId === 'string' ? obj.machineId : undefined
    const accessToken = typeof obj.accessToken === 'string' ? obj.accessToken : undefined
    const profileArn = typeof obj.profileArn === 'string' ? obj.profileArn : undefined
    const expiresAt =
      typeof obj.expiresAt === 'string' || typeof obj.expiresAt === 'number'
        ? (obj.expiresAt as string | number)
        : undefined
    const clientId = typeof obj.clientId === 'string' ? obj.clientId : undefined
    const clientSecret = typeof obj.clientSecret === 'string' ? obj.clientSecret : undefined
    const region = typeof obj.region === 'string' ? obj.region : undefined
    const authMethod = typeof obj.authMethod === 'string' ? obj.authMethod : undefined
    const provider = typeof obj.provider === 'string' ? obj.provider : undefined
    const startUrl = typeof obj.startUrl === 'string' ? obj.startUrl : undefined
    const tokenEndpoint = typeof obj.tokenEndpoint === 'string' ? obj.tokenEndpoint : undefined
    const issuerUrl = typeof obj.issuerUrl === 'string' ? obj.issuerUrl : undefined
    const scopes = typeof obj.scopes === 'string' ? obj.scopes : undefined

    return {
      email,
      subscriptionTitle: importedSubscriptionTitle(obj),
      userId,
      nickname,
      idp,
      status,
      machineId,
      traffic: obj.traffic,
      credentials: {
        refreshToken: obj.refreshToken,
        accessToken,
        profileArn,
        expiresAt,
        clientId,
        clientSecret,
        region,
        authMethod,
        provider,
        startUrl,
        tokenEndpoint,
        issuerUrl,
        scopes,
      },
    }
  }
  return item
}
